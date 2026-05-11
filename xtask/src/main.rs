//! Q-COLD repository-local task-flow adapter.

#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

mod quality;

const DEFAULT_PAUSED_TASK_TTL_HOURS: u64 = 2;
const DEFAULT_BUNDLE_RETENTION_HOURS: u64 = 24;

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8> {
    let cli = Cli::parse();
    match cli.command {
        CommandSet::Task(args) => task_command(args),
        CommandSet::Build(args) => run_status("cargo", cargo_args("build", &args.args)),
        CommandSet::Install(args) => install_command(&args.args),
        CommandSet::Verify(args) | CommandSet::Ci(args) => verify_command(&args.args),
        CommandSet::Compat(args) => Ok(not_applicable("compat", &args.args)),
        CommandSet::Ffi(args) => Ok(not_applicable("ffi", &args.args)),
    }
}

#[derive(Parser)]
#[command(name = "xtask", about = "Q-COLD repository-local task-flow adapter")]
struct Cli {
    #[command(subcommand)]
    command: CommandSet,
}

#[derive(Subcommand)]
enum CommandSet {
    Task(TaskArgs),
    Build(PassthroughArgs),
    Install(PassthroughArgs),
    Verify(PassthroughArgs),
    Ci(PassthroughArgs),
    Compat(PassthroughArgs),
    Ffi(PassthroughArgs),
}

#[derive(Args)]
struct PassthroughArgs {
    #[arg(trailing_var_arg = true)]
    args: Vec<OsString>,
}

#[derive(Args)]
struct TaskArgs {
    #[command(subcommand)]
    command: TaskCommand,
}

#[derive(Subcommand)]
enum TaskCommand {
    Inspect {
        topic: Option<String>,
    },
    Open {
        task_slug: String,
        profile: Option<String>,
    },
    Enter,
    List,
    TerminalCheck,
    IterationNotify {
        #[arg(long)]
        message: String,
    },
    Finalize {
        #[arg(long)]
        message: String,
    },
    Pause {
        #[arg(long)]
        reason: String,
    },
    Closeout(CloseoutArgs),
    Bundle {
        task_id: Option<String>,
    },
    Clean {
        task_slug: String,
    },
    Clear {
        task_slug: String,
    },
    ClearAll,
    OrphanList,
    OrphanClearStale {
        #[arg(long)]
        max_age_hours: u64,
    },
}

#[derive(Args)]
struct CloseoutArgs {
    #[arg(long)]
    outcome: String,
    #[arg(long)]
    message: Option<String>,
    #[arg(long)]
    reason: Option<String>,
}

fn task_command(args: TaskArgs) -> Result<u8> {
    match args.command {
        TaskCommand::Inspect { topic } => inspect_command(topic.as_deref()),
        TaskCommand::Open { task_slug, profile } => open_command(&task_slug, profile.as_deref()),
        TaskCommand::Enter => enter_command(),
        TaskCommand::List => list_command(),
        TaskCommand::TerminalCheck => terminal_check_command(),
        TaskCommand::IterationNotify { message } => append_event_command("iteration", &message),
        TaskCommand::Finalize { message } => append_event_command("finalize", &message),
        TaskCommand::Pause { reason } => pause_command(&reason),
        TaskCommand::Closeout(args) => closeout_command(&args),
        TaskCommand::Bundle { task_id } => bundle_command(task_id.as_deref()),
        TaskCommand::Clean { task_slug } => clean_command(&task_slug, false),
        TaskCommand::Clear { task_slug } => clean_command(&task_slug, true),
        TaskCommand::ClearAll => clear_all_command(),
        TaskCommand::OrphanList => Ok(orphan_list_command()),
        TaskCommand::OrphanClearStale { max_age_hours } => {
            orphan_clear_stale_command(max_age_hours)
        }
    }
}

fn inspect_command(topic: Option<&str>) -> Result<u8> {
    let repo = repo_root()?;
    println!("[task-inspect] primary action=sync");
    println!("[task-inspect] ready path={}", repo.display());
    println!("[task-inspect] closeout=not-required");
    if let Some(topic) = topic {
        println!("[task-inspect] topic={topic}");
    }
    println!("[task-inspect] mode=read-only no-worktree no-devcontainer");
    Ok(0)
}

fn open_command(task_slug: &str, profile: Option<&str>) -> Result<u8> {
    ensure_slug(task_slug)?;
    let repo = repo_root()?;
    if let Some(mut task) = find_task(&repo, task_slug)? {
        if task.status == "paused" || task.status == "failed-closeout" {
            task.status = "open".to_string();
            task.updated_at = unix_now().to_string();
            if let Some(profile) = profile {
                task.task_profile = profile.to_string();
            }
            write_task_env(&task)?;
            append_event(&task.task_worktree, "task-resume", "resumed task")?;
            println!(
                "task-resumed\t{task_slug}\t{}",
                task.task_worktree.display()
            );
            println!("TASK_WORKTREE={}", task.task_worktree.display());
            return Ok(0);
        }
    }
    ensure_clean(&repo, "primary checkout")?;
    let base_head = git_output(&repo, ["rev-parse", "HEAD"])?;
    let base_branch = git_output(&repo, ["branch", "--show-current"])?;
    let branch = format!("task/{task_slug}");
    let execution_anchor = task_execution_anchor();
    let worktree = managed_root(&repo)?.join(format!("{execution_anchor}-{task_slug}"));
    if worktree.exists() {
        bail!(
            "managed task worktree already exists: {}",
            worktree.display()
        );
    }
    fs::create_dir_all(
        worktree
            .parent()
            .context("managed worktree has no parent")?,
    )?;
    run_git(
        &repo,
        [
            "worktree",
            "add",
            "-b",
            &branch,
            path_arg(&worktree),
            "HEAD",
        ],
    )?;

    let task = TaskEnv {
        task_id: branch.clone(),
        task_name: task_slug.to_string(),
        task_branch: branch.clone(),
        task_execution_anchor: execution_anchor,
        task_description: task_open_description(task_slug),
        task_worktree: worktree.clone(),
        task_profile: profile.unwrap_or("default").to_string(),
        primary_repo_path: repo,
        base_branch,
        base_head,
        task_head: git_output(&worktree, ["rev-parse", "HEAD"])?,
        started_at: unix_now().to_string(),
        status: "open".to_string(),
        updated_at: unix_now().to_string(),
        devcontainer_name: "host-shell".to_string(),
        delivery_mode: "self-hosted-qcold".to_string(),
    };
    write_task_env(&task)?;
    append_event(&worktree, "task-open", &format!("opened {branch}"))?;
    println!("task-opened\t{task_slug}\t{}", worktree.display());
    println!("TASK_WORKTREE={}", worktree.display());
    Ok(0)
}

fn task_open_description(task_slug: &str) -> String {
    std::env::var("QCOLD_TASKFLOW_PROMPT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("Q-COLD self-hosted task {task_slug}"))
}

fn enter_command() -> Result<u8> {
    let task = current_task_env()?;
    println!("task-enter\t{}", task.task_worktree.display());
    println!(
        "cd {}",
        shell_quote(&task.task_worktree.display().to_string())
    );
    Ok(0)
}

fn list_command() -> Result<u8> {
    let tasks = open_tasks(&task_inventory_repo_root()?)?;
    println!("tasks\tcount={}", tasks.len());
    for task in tasks {
        println!(
            "task\t{}\tstatus={}\tworktree={}",
            task.task_name,
            task.status,
            task.task_worktree.display()
        );
    }
    Ok(0)
}

fn terminal_check_command() -> Result<u8> {
    let repo = task_inventory_repo_root()?;
    let cleanup = clear_stale_paused_tasks(&repo, paused_task_ttl_hours()?)?;
    let bundle_cleanup = clear_stale_bundles(&repo, bundle_retention_hours()?)?;
    if cleanup.removed > 0 {
        println!(
            "paused-task-cleanup\tmax_age_hours={}\tremoved={}",
            cleanup.max_age_hours, cleanup.removed
        );
    }
    if bundle_cleanup.removed > 0 {
        println!(
            "bundle-cleanup\tretention_hours={}\tremoved={}",
            bundle_cleanup.retention_hours, bundle_cleanup.removed
        );
    }
    let tasks = open_tasks(&repo)?
        .into_iter()
        .filter(|task| task_blocks_terminal(&task.status))
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        let branch = git_output(&repo, ["branch", "--show-current"]).unwrap_or_default();
        println!("terminal-ok\t{}\t{}", repo.display(), branch);
        return Ok(0);
    }
    let primary_dirty = dirty_paths(&repo)?;
    for path in &primary_dirty {
        println!("primary-dirty-file\t{}", path.display());
    }
    let mut incomplete_closeout = false;
    for task in &tasks {
        println!(
            "open-task\t{}\t{}",
            task.task_name,
            task.task_worktree.display()
        );
        for path in dirty_paths(&task.task_worktree)?.intersection(&primary_dirty) {
            println!(
                "open-task-dirty-overlap\t{}\t{}\t{}",
                task.task_name,
                path.display(),
                task.task_worktree.display()
            );
        }
        if task.status == "failed-closeout" {
            incomplete_closeout = true;
            println!(
                "incomplete-task\t{}\t{}\t{}",
                task.task_name,
                task.status,
                task.task_worktree.display()
            );
        } else if task.status == "paused" {
            println!(
                "paused-task\t{}\t{}\t{}",
                task.task_name,
                task.status,
                task.task_worktree.display()
            );
        }
    }
    if incomplete_closeout {
        eprintln!("terminal-check blocked: incomplete failed-closeout task state remains");
    } else if tasks.iter().any(|task| task.status == "paused") {
        eprintln!("terminal-check blocked: paused managed task state remains");
    } else {
        eprintln!("terminal-check blocked: managed task worktrees remain open");
    }
    Ok(1)
}

fn dirty_paths(repo: &Path) -> Result<BTreeSet<PathBuf>> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()
        .context("failed to inspect git status")?;
    if !output.status.success() {
        bail!("git status failed with status {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(status_path)
        .collect())
}

fn status_path(line: &str) -> Option<PathBuf> {
    let path = line.get(3..)?.split(" -> ").last().unwrap_or_default();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn append_event_command(kind: &str, message: &str) -> Result<u8> {
    let task = current_task_env()?;
    append_event(&task.task_worktree, kind, message)?;
    println!("task-event\t{kind}\t{}", task.task_name);
    Ok(0)
}

fn pause_command(reason: &str) -> Result<u8> {
    let mut task = current_task_env()?;
    task.status = "paused".to_string();
    task.updated_at = unix_now().to_string();
    write_task_env(&task)?;
    append_event(&task.task_worktree, "task-pause", reason)?;
    println!("task-pause\t{}", task.task_name);
    println!("REASON={reason}");
    println!("TASK_WORKTREE={}", task.task_worktree.display());
    Ok(0)
}

fn closeout_command(args: &CloseoutArgs) -> Result<u8> {
    if std::env::var("QCOLD_TASKFLOW_CONTEXT").as_deref() == Ok("managed-task-devcontainer") {
        bail!(
            "task closeout must be launched from the host-side managed task worktree shell\n\
             closeout has not started, so no manual bundle is needed for this preflight error"
        );
    }
    let mut task = current_task_env().context(
        "closeout has not started, so no manual bundle is needed for this preflight error",
    )?;
    match args.outcome.as_str() {
        "success" => closeout_success(&mut task, args.message.as_deref()),
        "blocked" => closeout_non_success(&mut task, "blocked", args.reason.as_deref(), 10),
        "failed" => closeout_non_success(&mut task, "failed", args.reason.as_deref(), 11),
        outcome => bail!("unsupported closeout outcome: {outcome}"),
    }
}

fn closeout_success(task: &mut TaskEnv, message: Option<&str>) -> Result<u8> {
    let message = message.context("--message is required for success closeout")?;
    let agent_worktree = agent_return_worktree();
    ensure_clean(&task.primary_repo_path, "primary checkout")?;
    run_preflight(PreflightProfile::default())?;
    if !git_output(&task.task_worktree, ["status", "--porcelain"])?.is_empty() {
        run_git(&task.task_worktree, ["add", "-A"])?;
        run_git(&task.task_worktree, ["commit", "-m", message])?;
    }
    deliver_task_branch_to_primary(task)?;
    task.status = "closed:success".to_string();
    task.updated_at = unix_now().to_string();
    write_task_env(task)?;
    append_event(&task.task_worktree, "task-closeout", "success")?;
    let worktree = task.task_worktree.clone();
    let branch = task.task_branch.clone();
    if let Some(agent_worktree) = agent_worktree {
        run_git(&worktree, ["checkout", "--detach"])?;
        let task_state = worktree.join(".task");
        if task_state.exists() {
            fs::remove_dir_all(&task_state)
                .with_context(|| format!("failed to remove {}", task_state.display()))?;
        }
        run_git(&task.primary_repo_path, ["branch", "-d", &branch])?;
        println!("task-closeout\tsuccess\t{}", task.task_name);
        println!("task-return\t{}", agent_worktree.display());
        println!("QCOLD_AGENT_WORKTREE={}", agent_worktree.display());
        return Ok(0);
    }
    run_git(
        &task.primary_repo_path,
        ["worktree", "remove", path_arg(&worktree)],
    )?;
    run_git(&task.primary_repo_path, ["branch", "-d", &branch])?;
    println!("task-closeout\tsuccess\t{}", task.task_name);
    Ok(0)
}

fn deliver_task_branch_to_primary(task: &TaskEnv) -> Result<()> {
    run_git(&task.primary_repo_path, ["fetch", "origin"])?;
    let remote_base = format!("refs/remotes/origin/{}", task.base_branch);
    run_git(
        &task.primary_repo_path,
        ["merge", "--ff-only", &remote_base],
    )?;
    run_git(&task.task_worktree, ["rebase", &remote_base])?;
    run_git(
        &task.primary_repo_path,
        ["merge", "--ff-only", &task.task_branch],
    )?;
    run_git(
        &task.primary_repo_path,
        ["push", "origin", &task.base_branch],
    )?;
    run_git(&task.primary_repo_path, ["fetch", "origin"])?;
    Ok(())
}

fn agent_return_worktree() -> Option<PathBuf> {
    let value = std::env::var("QCOLD_AGENT_WORKTREE").ok()?;
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

fn closeout_non_success(
    task: &mut TaskEnv,
    outcome: &str,
    reason: Option<&str>,
    code: u8,
) -> Result<u8> {
    task.status = format!("closed:{outcome}");
    task.updated_at = unix_now().to_string();
    write_task_env(task)?;
    append_event(
        &task.task_worktree,
        "task-closeout",
        reason.unwrap_or(outcome),
    )?;
    let bundle = create_task_archive_bundle(task)?;
    let primary_clean = git_output(&task.primary_repo_path, ["status", "--porcelain"])?.is_empty();
    std::env::set_current_dir(&task.primary_repo_path).with_context(|| {
        format!(
            "failed to leave task worktree for cleanup: {}",
            task.primary_repo_path.display()
        )
    })?;
    let worktree_removed = git_status(
        &task.primary_repo_path,
        [
            "worktree",
            "remove",
            "--force",
            path_arg(&task.task_worktree),
        ],
    )?;
    let branch_removed = git_status(&task.primary_repo_path, ["branch", "-D", &task.task_branch])?;
    let receipt = TerminalReceipt {
        outcome,
        reason,
        primary_clean,
        worktree_removed,
        branch_removed,
    };
    add_terminal_receipt_to_bundle(&bundle, &receipt)?;
    println!("task-closeout\t{outcome}\t{}", task.task_name);
    if let Some(reason) = reason {
        println!("REASON={reason}");
    }
    println!("BUNDLE_PATH={}", bundle.display());
    Ok(code)
}

struct TerminalReceipt<'a> {
    outcome: &'a str,
    reason: Option<&'a str>,
    primary_clean: bool,
    worktree_removed: bool,
    branch_removed: bool,
}

fn create_task_archive_bundle(task: &TaskEnv) -> Result<PathBuf> {
    let bundles = task.primary_repo_path.join("bundles");
    fs::create_dir_all(&bundles)?;
    let bundle = bundles.join(format!("{}-{}.zip", task.task_name, unix_now()));
    let status = Command::new("7z")
        .current_dir(&task.task_worktree)
        .args(["a", path_arg(&bundle), ".", "-xr!.git"])
        .status()
        .context("failed to create terminal evidence bundle")?;
    if !status.success() {
        bail!("7z failed to create terminal evidence bundle with status {status}");
    }
    Ok(bundle)
}

fn add_terminal_receipt_to_bundle(bundle: &Path, receipt: &TerminalReceipt<'_>) -> Result<()> {
    let staging = std::env::temp_dir().join(format!(
        "qcold-terminal-receipt-{}-{}",
        std::process::id(),
        unix_now()
    ));
    let metadata = staging.join("metadata");
    fs::create_dir_all(&metadata)?;
    fs::write(
        metadata.join("terminal-receipt.env"),
        render_terminal_receipt(receipt),
    )?;
    let status = Command::new("7z")
        .current_dir(&staging)
        .args(["a", path_arg(bundle), "metadata/terminal-receipt.env"])
        .status()
        .context("failed to append terminal receipt to bundle")?;
    fs::remove_dir_all(&staging).ok();
    if !status.success() {
        bail!("7z failed to append terminal receipt with status {status}");
    }
    Ok(())
}

fn render_terminal_receipt(receipt: &TerminalReceipt<'_>) -> String {
    let mut output = String::new();
    for (key, value) in [
        ("OUTCOME", receipt.outcome),
        ("REASON", receipt.reason.unwrap_or("")),
        (
            "PRIMARY_CHECKOUT_CLEAN",
            if receipt.primary_clean { "yes" } else { "no" },
        ),
        (
            "TASK_WORKTREE_REMOVED",
            if receipt.worktree_removed {
                "yes"
            } else {
                "no"
            },
        ),
        (
            "LOCAL_TASK_BRANCH_REMOVED",
            if receipt.branch_removed { "yes" } else { "no" },
        ),
        ("CANONICAL_VALIDATION", "not-applicable"),
    ] {
        output.push_str(key);
        output.push('=');
        output.push_str(&shell_quote(value));
        output.push('\n');
    }
    output
}

fn bundle_command(task_id: Option<&str>) -> Result<u8> {
    let repo = repo_root()?;
    let bundles = repo.join("bundles");
    fs::create_dir_all(&bundles)?;
    let name = task_id.unwrap_or("source").replace(['/', '\\'], "-");
    let bundle = bundles.join(format!("{name}-{}.zip", unix_now()));
    run_git(
        &repo,
        ["archive", "--format=zip", "-o", path_arg(&bundle), "HEAD"],
    )?;
    println!("BUNDLE_PATH={}", bundle.display());
    Ok(0)
}

fn clean_command(task_slug: &str, force: bool) -> Result<u8> {
    let repo = task_inventory_repo_root()?;
    let Some(task) = find_task(&repo, task_slug)? else {
        println!("task-clear\tmissing task={task_slug}");
        return Ok(0);
    };
    if !force && !git_output(&task.task_worktree, ["status", "--porcelain"])?.is_empty() {
        bail!("dirty task worktree: {}", task.task_worktree.display());
    }
    run_git(
        &repo,
        [
            "worktree",
            "remove",
            "--force",
            path_arg(&task.task_worktree),
        ],
    )?;
    let _ = run_git(&repo, ["branch", "-D", &task.task_branch]);
    println!("[task-clear] cleared task={}", task.task_branch);
    Ok(0)
}

fn clear_all_command() -> Result<u8> {
    let repo = task_inventory_repo_root()?;
    for task in open_tasks(&repo)? {
        run_git(
            &repo,
            [
                "worktree",
                "remove",
                "--force",
                path_arg(&task.task_worktree),
            ],
        )?;
        let _ = run_git(&repo, ["branch", "-D", &task.task_branch]);
    }
    println!("task-clear-all\tok");
    Ok(0)
}

fn orphan_list_command() -> u8 {
    println!("orphans\tcount=0");
    0
}

fn orphan_clear_stale_command(max_age_hours: u64) -> Result<u8> {
    let repo = task_inventory_repo_root()?;
    let cleanup = clear_stale_paused_tasks(&repo, max_age_hours)?;
    let bundle_cleanup = clear_stale_bundles(&repo, bundle_retention_hours()?)?;
    println!(
        "orphan-clear-stale\tmax_age_hours={}\tremoved={}",
        cleanup.max_age_hours, cleanup.removed
    );
    println!(
        "bundle-clear-stale\tretention_hours={}\tremoved={}",
        bundle_cleanup.retention_hours, bundle_cleanup.removed
    );
    Ok(0)
}

struct StaleCleanup {
    max_age_hours: u64,
    removed: usize,
}

struct BundleCleanup {
    retention_hours: u64,
    removed: usize,
}

include!("task/cleanup.rs");
include!("task/verify.rs");
include!("task/env_io.rs");
include!("task/tests.rs");
