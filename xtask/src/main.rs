//! Q-COLD repository-local task-flow adapter.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};

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
        TaskCommand::Closeout(args) => closeout_command(&args),
        TaskCommand::Bundle { task_id } => bundle_command(task_id.as_deref()),
        TaskCommand::Clean { task_slug } => clean_command(&task_slug, false),
        TaskCommand::Clear { task_slug } => clean_command(&task_slug, true),
        TaskCommand::ClearAll => clear_all_command(),
        TaskCommand::OrphanList => Ok(orphan_list_command()),
        TaskCommand::OrphanClearStale { max_age_hours } => {
            println!("orphan-clear-stale\tmax_age_hours={max_age_hours}\tremoved=0");
            Ok(0)
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
        task_description: format!("Q-COLD self-hosted task {task_slug}"),
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
    let tasks = open_tasks(&repo)?;
    if tasks.is_empty() {
        let branch = git_output(&repo, ["branch", "--show-current"]).unwrap_or_default();
        println!("terminal-ok\t{}\t{}", repo.display(), branch);
        return Ok(0);
    }
    for task in &tasks {
        println!(
            "open-task\t{}\t{}",
            task.task_name,
            task.task_worktree.display()
        );
    }
    eprintln!("terminal-check blocked: managed task worktrees remain open");
    Ok(1)
}

fn append_event_command(kind: &str, message: &str) -> Result<u8> {
    let task = current_task_env()?;
    append_event(&task.task_worktree, kind, message)?;
    println!("task-event\t{kind}\t{}", task.task_name);
    Ok(0)
}

fn closeout_command(args: &CloseoutArgs) -> Result<u8> {
    if std::env::var("QCOLD_TASKFLOW_CONTEXT").as_deref() == Ok("managed-task-devcontainer") {
        bail!(
            "task closeout must be launched from the host-side managed task worktree shell\ncloseout has not started, so no manual bundle is needed for this preflight error"
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
    run_required("cargo", ["fmt", "--check"].map(OsString::from).to_vec())?;
    run_required(
        "cargo",
        [
            "test",
            "--bin",
            "cargo-qcold",
            "--locked",
            "--",
            "--test-threads=1",
        ]
        .map(OsString::from)
        .to_vec(),
    )?;
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
    println!("task-closeout\t{outcome}\t{}", task.task_name);
    if let Some(reason) = reason {
        println!("REASON={reason}");
    }
    Ok(code)
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

fn verify_command(args: &[OsString]) -> Result<u8> {
    if !args.is_empty() {
        println!("verify-profile\t{}", display_args(args));
    }
    run_required("cargo", ["fmt", "--check"].map(OsString::from).to_vec())?;
    run_required(
        "cargo",
        [
            "test",
            "--bin",
            "cargo-qcold",
            "--locked",
            "--",
            "--test-threads=1",
        ]
        .map(OsString::from)
        .to_vec(),
    )?;
    Ok(0)
}

fn install_command(args: &[OsString]) -> Result<u8> {
    let mut cargo_args = vec![
        OsString::from("install"),
        OsString::from("--path"),
        OsString::from("."),
        OsString::from("--locked"),
    ];
    cargo_args.extend(args.iter().cloned());
    run_status("cargo", cargo_args)
}

fn not_applicable(kind: &str, args: &[OsString]) -> u8 {
    println!("{kind}\tnot-applicable\t{}", display_args(args));
    0
}

fn cargo_args(command: &str, extra: &[OsString]) -> Vec<OsString> {
    let mut args = vec![OsString::from(command), OsString::from("--locked")];
    args.extend(extra.iter().cloned());
    args
}

#[derive(Default)]
struct TaskEnv {
    task_id: String,
    task_name: String,
    task_branch: String,
    task_execution_anchor: String,
    task_description: String,
    task_worktree: PathBuf,
    task_profile: String,
    primary_repo_path: PathBuf,
    base_branch: String,
    base_head: String,
    task_head: String,
    started_at: String,
    status: String,
    updated_at: String,
    devcontainer_name: String,
    delivery_mode: String,
}

fn current_task_env() -> Result<TaskEnv> {
    let root = repo_root()?;
    let env_path = root.join(".task/task.env");
    if !env_path.is_file() {
        bail!("run this from a managed task worktree");
    }
    parse_task_env(&env_path)
}

fn open_tasks(repo: &Path) -> Result<Vec<TaskEnv>> {
    let root = managed_root(repo)?;
    let mut tasks = Vec::new();
    if !root.exists() {
        return Ok(tasks);
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let env_path = entry.path().join(".task/task.env");
        if env_path.is_file() {
            tasks.push(parse_task_env(&env_path)?);
        }
    }
    tasks.sort_by(|left, right| left.task_name.cmp(&right.task_name));
    Ok(tasks)
}

fn find_task(repo: &Path, task_slug: &str) -> Result<Option<TaskEnv>> {
    Ok(open_tasks(repo)?.into_iter().find(|task| {
        task.task_name == task_slug
            || task.task_branch == format!("task/{task_slug}")
            || task
                .task_worktree
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(task_slug))
    }))
}

fn parse_task_env(path: &Path) -> Result<TaskEnv> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let value = |key: &str| {
        content
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(unquote)
            .unwrap_or_default()
    };
    Ok(TaskEnv {
        task_id: value("TASK_ID"),
        task_name: value("TASK_NAME"),
        task_branch: value("TASK_BRANCH"),
        task_execution_anchor: value("TASK_EXECUTION_ANCHOR"),
        task_description: value("TASK_DESCRIPTION"),
        task_worktree: PathBuf::from(value("TASK_WORKTREE")),
        task_profile: value("TASK_PROFILE"),
        primary_repo_path: PathBuf::from(value("PRIMARY_REPO_PATH")),
        base_branch: value("BASE_BRANCH"),
        base_head: value("BASE_HEAD"),
        task_head: value("TASK_HEAD"),
        started_at: value("STARTED_AT"),
        status: value("STATUS"),
        updated_at: value("UPDATED_AT"),
        devcontainer_name: value("DEVCONTAINER_NAME"),
        delivery_mode: value("DELIVERY_MODE"),
    })
}

fn write_task_env(task: &TaskEnv) -> Result<()> {
    let dir = task.task_worktree.join(".task/logs");
    fs::create_dir_all(&dir)?;
    let env_path = task.task_worktree.join(".task/task.env");
    let fields = [
        ("TASK_ID", task.task_id.as_str()),
        ("TASK_NAME", task.task_name.as_str()),
        ("TASK_BRANCH", task.task_branch.as_str()),
        ("TASK_EXECUTION_ANCHOR", task.task_execution_anchor.as_str()),
        ("TASK_DESCRIPTION", task.task_description.as_str()),
        ("TASK_WORKTREE", &task.task_worktree.display().to_string()),
        ("TASK_PROFILE", task.task_profile.as_str()),
        (
            "PRIMARY_REPO_PATH",
            &task.primary_repo_path.display().to_string(),
        ),
        ("BASE_BRANCH", task.base_branch.as_str()),
        ("BASE_HEAD", task.base_head.as_str()),
        ("TASK_HEAD", task.task_head.as_str()),
        ("STARTED_AT", task.started_at.as_str()),
        ("STATUS", task.status.as_str()),
        ("UPDATED_AT", task.updated_at.as_str()),
        ("DEVCONTAINER_NAME", task.devcontainer_name.as_str()),
        ("DELIVERY_MODE", task.delivery_mode.as_str()),
    ];
    let mut output = String::new();
    for (key, value) in fields {
        output.push_str(key);
        output.push('=');
        output.push_str(&shell_quote(value));
        output.push('\n');
    }
    fs::write(env_path, output)?;
    Ok(())
}

fn append_event(worktree: &Path, kind: &str, message: &str) -> Result<()> {
    let path = worktree.join(".task/logs/events.ndjson");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let event = format!(
        "{{\"kind\":\"{}\",\"message\":\"{}\",\"timestamp\":{}}}\n",
        json_escape(kind),
        json_escape(message),
        unix_now()
    );
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all_ext(event.as_bytes())
}

trait WriteAllExt {
    fn write_all_ext(self, bytes: &[u8]) -> Result<()>;
}

impl WriteAllExt for fs::File {
    fn write_all_ext(mut self, bytes: &[u8]) -> Result<()> {
        use std::io::Write as _;

        self.write_all(bytes)?;
        Ok(())
    }
}

fn repo_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to locate git root")?;
    if !output.status.success() {
        bail!("not inside a git checkout");
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string()).canonicalize()?)
}

fn task_inventory_repo_root() -> Result<PathBuf> {
    let root = repo_root()?;
    let env_path = root.join(".task/task.env");
    if !env_path.is_file() {
        return Ok(root);
    }
    let task = parse_task_env(&env_path)?;
    if task.primary_repo_path.as_os_str().is_empty() {
        return Ok(root);
    }
    Ok(task
        .primary_repo_path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", task.primary_repo_path.display()))?)
}

fn managed_root(repo: &Path) -> Result<PathBuf> {
    Ok(repo
        .parent()
        .context("repository root has no parent")?
        .join("WT")
        .join(repo.file_name().context("repository root has no name")?))
}

fn ensure_clean(repo: &Path, label: &str) -> Result<()> {
    let status = git_output(repo, ["status", "--porcelain"])?;
    if status.is_empty() {
        Ok(())
    } else {
        bail!("{label} is dirty:\n{status}")
    }
}

fn ensure_slug(slug: &str) -> Result<()> {
    if slug.is_empty()
        || slug.starts_with('-')
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        bail!("task slug must contain only ASCII letters, digits, '-' or '_'");
    }
    Ok(())
}

fn run_git<const N: usize>(repo: &Path, args: [&str; N]) -> Result<()> {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .context("failed to run git")?;
    if !status.success() {
        bail!("git command failed");
    }
    Ok(())
}

fn git_output<const N: usize>(repo: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        bail!("git command failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_status(program: &str, args: Vec<OsString>) -> Result<u8> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {program}"))?;
    Ok(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1))
}

fn run_required(program: &str, args: Vec<OsString>) -> Result<()> {
    let code = run_status(program, args)?;
    if code == 0 {
        Ok(())
    } else {
        bail!("{program} validation failed with code {code}");
    }
}

fn task_execution_anchor() -> String {
    std::env::var("QCOLD_TASK_SEQUENCE")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(sequence_anchor)
        .unwrap_or_else(short_anchor)
}

fn sequence_anchor(sequence: u64) -> Option<String> {
    (sequence > 0).then(|| format!("{sequence:03}"))
}

fn short_anchor() -> String {
    format!("{:x}", unix_now())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn path_arg(path: &Path) -> &str {
    path.to_str().unwrap_or("")
}

fn display_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "-_./:".contains(ch))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_anchor_is_zero_padded_operator_order() {
        assert_eq!(sequence_anchor(1).as_deref(), Some("001"));
        assert_eq!(sequence_anchor(42).as_deref(), Some("042"));
        assert_eq!(sequence_anchor(1001).as_deref(), Some("1001"));
        assert_eq!(sequence_anchor(0), None);
    }

    #[test]
    fn agent_return_worktree_reads_nonempty_env() {
        std::env::remove_var("QCOLD_AGENT_WORKTREE");
        assert_eq!(agent_return_worktree(), None);

        std::env::set_var("QCOLD_AGENT_WORKTREE", "  ");
        assert_eq!(agent_return_worktree(), None);

        std::env::set_var("QCOLD_AGENT_WORKTREE", "/workspace/WT/repo/agents/c1");
        assert_eq!(
            agent_return_worktree().as_deref(),
            Some(Path::new("/workspace/WT/repo/agents/c1"))
        );
        std::env::remove_var("QCOLD_AGENT_WORKTREE");
    }

    #[test]
    fn deliver_task_branch_pushes_base_and_refreshes_origin_tracking() {
        let root = unique_test_dir("qcold-self-closeout-push");
        let remote = root.join("remote.git");
        let primary = root.join("primary");

        run_git_in(&root, ["init", "--bare", path_arg(&remote)]);
        run_git_in(&root, ["clone", path_arg(&remote), path_arg(&primary)]);
        run_git_in(&primary, ["config", "user.name", "tester"]);
        run_git_in(&primary, ["config", "user.email", "tester@example.com"]);
        run_git_in(&primary, ["checkout", "-B", "main"]);
        fs::write(primary.join("README.md"), "seed\n").unwrap();
        run_git_in(&primary, ["add", "README.md"]);
        run_git_in(&primary, ["commit", "-m", "seed"]);
        run_git_in(&primary, ["push", "-u", "origin", "main"]);

        let worktree = root.join("task");
        run_git_in(
            &primary,
            [
                "worktree",
                "add",
                "-b",
                "task/push-proof",
                path_arg(&worktree),
                "HEAD",
            ],
        );
        fs::write(worktree.join("proof.txt"), "proof\n").unwrap();
        run_git_in(&worktree, ["add", "proof.txt"]);
        run_git_in(&worktree, ["commit", "-m", "add proof"]);

        let task = TaskEnv {
            task_id: "task/push-proof".into(),
            task_name: "push-proof".into(),
            task_branch: "task/push-proof".into(),
            task_execution_anchor: "001".into(),
            task_description: "push proof".into(),
            task_worktree: worktree,
            task_profile: "default".into(),
            primary_repo_path: primary.clone(),
            base_branch: "main".into(),
            base_head: git_output(&primary, ["rev-parse", "main"]).unwrap(),
            task_head: String::new(),
            started_at: "1".into(),
            status: "open".into(),
            updated_at: "1".into(),
            devcontainer_name: "host-shell".into(),
            delivery_mode: "self-hosted-qcold".into(),
        };

        deliver_task_branch_to_primary(&task).unwrap();

        let local_main = git_output(&primary, ["rev-parse", "main"]).unwrap();
        let origin_main = git_output(&primary, ["rev-parse", "origin/main"]).unwrap();
        let remote_main = git_output(&remote, ["rev-parse", "refs/heads/main"]).unwrap();

        assert_eq!(local_main, origin_main);
        assert_eq!(local_main, remote_main);
        assert_eq!(
            fs::read_to_string(primary.join("proof.txt")).unwrap(),
            "proof\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("{name}-{}-{}", std::process::id(), unix_now()));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run_git_in<const N: usize>(repo: &Path, args: [&str; N]) {
        run_git(repo, args).unwrap();
    }
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("'\\''", "'")
    } else {
        value.to_string()
    }
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
