use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::state;

const AGENT_DISPLAY_NAMES: &[&str] = &[
    "Socrates",
    "Plato",
    "Aristotle",
    "Diogenes",
    "Epicurus",
    "Zeno",
    "Thales",
    "Pythagoras",
    "Democritus",
    "Heraclitus",
];

const KNOWN_AGENT_COMMANDS: &[(&str, &str, AgentInvocation)] = &[
    ("c1", "Codex account 1", AgentInvocation::Exec),
    ("cc1", "Codex account 1 compact", AgentInvocation::Direct),
    ("c2", "Codex account 2", AgentInvocation::Direct),
    ("cc2", "Codex account 2 compact", AgentInvocation::Direct),
    ("codex", "Codex default", AgentInvocation::Exec),
];

#[derive(Clone, Copy)]
enum AgentInvocation {
    Exec,
    Direct,
}

impl AgentInvocation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Exec => "exec",
            Self::Direct => "direct",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AvailableAgentCommand {
    pub command: String,
    pub label: String,
    pub invocation: &'static str,
    pub path: String,
    pub account: String,
    pub status_command: String,
}

#[derive(Clone, Copy)]
enum TerminalBackend {
    Tmux,
    Zellij,
}

enum TerminalTarget {
    Tmux { session: String },
    Zellij { session: String, pane: String },
}

struct Launch {
    command: Vec<String>,
    cwd: PathBuf,
    qcold_repo_root: Option<PathBuf>,
    qcold_agent_worktree: Option<PathBuf>,
}

struct TerminalLaunch {
    command: String,
    cwd: PathBuf,
    qcold_repo_root: Option<PathBuf>,
    qcold_agent_worktree: Option<PathBuf>,
}

struct LaunchContext {
    cwd: PathBuf,
    qcold_repo_root: Option<PathBuf>,
    qcold_agent_worktree: Option<PathBuf>,
}

#[derive(Args)]
pub struct AgentArgs {
    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Subcommand)]
enum AgentCommand {
    #[command(about = "Start an agent process under Q-COLD tracking")]
    Start(StartArgs),
    #[command(about = "List tracked agent processes")]
    List,
}

#[derive(Args)]
struct StartArgs {
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    track: String,
    #[arg(long, help = "Directory used as the agent launch context")]
    cwd: Option<PathBuf>,
    #[arg(long, help = "Run the agent in an attachable tmux terminal session")]
    terminal: bool,
    #[arg(long, help = "Attach to the tmux terminal after starting the agent")]
    attach: bool,
    #[arg(required = true, trailing_var_arg = true)]
    command: Vec<String>,
}

pub fn run(args: AgentArgs) -> Result<u8> {
    match args.command {
        AgentCommand::Start(args) => {
            let record = if args.terminal || args.attach {
                start_terminal_agent(args.id, &args.track, &shell_join(&args.command), args.cwd)?
            } else {
                start_agent(args.id, args.track, args.command, args.cwd)?
            };
            println!("{}", snapshot_line(&record));
            if args.attach {
                attach_terminal(&record)?;
            }
        }
        AgentCommand::List => print!("{}", snapshot()?),
    }
    Ok(0)
}

pub fn snapshot() -> Result<String> {
    let _ = crate::sync_codex_task_records();
    let state = AgentState::load()?;
    Ok(render_snapshot(&state.records, SnapshotScope::All))
}

pub fn running_snapshot() -> Result<String> {
    let _ = crate::sync_codex_task_records();
    let state = AgentState::load()?;
    Ok(render_snapshot(&state.records, SnapshotScope::RunningOnly))
}

pub fn available_agent_commands() -> Vec<AvailableAgentCommand> {
    let mut commands = Vec::new();
    let mut seen = HashSet::new();
    for (command, label, invocation) in KNOWN_AGENT_COMMANDS {
        if let Some(path) = command_path(command) {
            seen.insert((*command).to_string());
            commands.push(AvailableAgentCommand {
                command: (*command).to_string(),
                label: (*label).to_string(),
                invocation: invocation.as_str(),
                path: path.display().to_string(),
                account: agent_account_key(command),
                status_command: status_probe_command(command),
            });
        }
    }
    for command in discover_numbered_codex_commands() {
        if !seen.insert(command.clone()) {
            continue;
        }
        if let Some(path) = command_path(&command) {
            commands.push(AvailableAgentCommand {
                label: format!("Codex account {}", command.trim_start_matches("codex")),
                account: agent_account_key(&command),
                status_command: status_probe_command(&command),
                command,
                invocation: AgentInvocation::Exec.as_str(),
                path: path.display().to_string(),
            });
        }
    }
    commands.sort_by(|left, right| agent_command_sort_key(&left.command).cmp(&agent_command_sort_key(&right.command)));
    commands
}

fn render_snapshot(records: &[AgentRecord], scope: SnapshotScope) -> String {
    let metadata = terminal_metadata_by_target().unwrap_or_default();
    render_snapshot_with_metadata(records, scope, &metadata)
}

fn render_snapshot_with_metadata(
    records: &[AgentRecord],
    scope: SnapshotScope,
    metadata: &HashMap<String, state::TerminalMetadataRow>,
) -> String {
    let rendered = records
        .iter()
        .filter_map(|record| {
            let state = process_state(record.pid);
            if scope == SnapshotScope::RunningOnly && state != "running" {
                None
            } else {
                Some(render_record_with_state(record, metadata, state))
            }
        })
        .collect::<Vec<_>>();
    let mut lines = vec![format!("agents\tcount={}", rendered.len())];
    lines.extend(rendered);
    format!("{}\n", lines.join("\n"))
}

pub fn snapshot_line(record: &AgentRecord) -> String {
    let metadata = terminal_metadata_by_target().unwrap_or_default();
    render_record(record, &metadata)
}

pub fn terminal_contexts() -> Result<Vec<TerminalAgentContext>> {
    let _ = crate::sync_codex_task_records();
    Ok(AgentState::load()?
        .records
        .into_iter()
        .filter_map(|record| {
            let target = terminal_target(&record)?;
            let command = terminal_command_from_record(&record.command);
            let (session, pane, target) = match target {
                TerminalTarget::Tmux { session } => {
                    let target = format!("{session}:0.0");
                    (session, "0.0".to_string(), target)
                }
                TerminalTarget::Zellij { session, pane } => {
                    let target = format!("zellij:{session}:{pane}");
                    (session, pane, target)
                }
            };
            Some(TerminalAgentContext {
                id: record.id,
                track: record.track,
                session,
                pane,
                target,
                started_at: record.started_at,
                command,
            })
        })
        .collect())
}

pub fn start_shell_agent(track: &str, command: &str) -> Result<AgentRecord> {
    if command.trim().is_empty() {
        bail!("agent command is empty");
    }
    let cwd = None;
    start_agent(
        None,
        track.to_string(),
        vec!["sh".to_string(), "-c".to_string(), command.to_string()],
        cwd,
    )
}

pub fn start_terminal_shell_agent(track: &str, command: &str) -> Result<AgentRecord> {
    if command.trim().is_empty() {
        bail!("agent command is empty");
    }
    start_terminal_agent(None, track, command, None)
}

pub fn start_terminal_shell_agent_in_cwd(
    track: &str,
    command: &str,
    cwd: PathBuf,
) -> Result<AgentRecord> {
    if command.trim().is_empty() {
        bail!("agent command is empty");
    }
    start_terminal_agent(None, track, command, Some(cwd))
}

fn start_agent(
    id: Option<String>,
    track: String,
    command: Vec<String>,
    requested_cwd: Option<PathBuf>,
) -> Result<AgentRecord> {
    if track.trim().is_empty() {
        bail!("agent track is empty");
    }
    if command.is_empty() {
        bail!("agent command is empty");
    }

    let state = AgentState::load()?;
    let started_at = unix_now()?;
    let id = id.unwrap_or_else(|| format!("{}-{started_at}", sanitize_id(&track)));
    if state.records.iter().any(|record| record.id == id) {
        bail!("agent id already exists: {id}");
    }

    let state_dir = state_dir()?;
    fs::create_dir_all(state_dir.join("logs"))?;
    let stdout_log_path = log_path(&id, "out")?;
    let stderr_log_path = log_path(&id, "err")?;
    let stdout = log_file(&stdout_log_path)?;
    let stderr = log_file(&stderr_log_path)?;
    let launch = prepare_launch(&id, &track, started_at, requested_cwd.as_deref(), &command)?;
    let mut process = Command::new(&launch.command[0]);
    process.args(&launch.command[1..]);
    process.current_dir(&launch.cwd);
    apply_qcold_launch_env(
        &mut process,
        launch.qcold_repo_root.as_deref(),
        launch.qcold_agent_worktree.as_deref(),
    );
    let child = process
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("failed to start agent command: {}", command.join(" ")))?;

    let record = AgentRecord {
        id,
        track,
        pid: child.id(),
        started_at,
        command: launch.command,
        cwd: Some(launch.cwd),
    };
    state::insert_agent(&state::AgentRow {
        id: record.id.clone(),
        track: record.track.clone(),
        pid: record.pid,
        started_at: record.started_at,
        command: record.command.clone(),
        cwd: record.cwd.clone(),
        stdout_log_path: Some(stdout_log_path),
        stderr_log_path: Some(stderr_log_path),
    })?;
    crate::record_agent_task(&record)?;
    Ok(record)
}

fn start_terminal_agent(
    id: Option<String>,
    track: &str,
    command: &str,
    requested_cwd: Option<PathBuf>,
) -> Result<AgentRecord> {
    if track.trim().is_empty() {
        bail!("agent track is empty");
    }
    if command.trim().is_empty() {
        bail!("agent command is empty");
    }
    let state = AgentState::load()?;
    let started_at = unix_now()?;
    let id = id.unwrap_or_else(|| format!("{}-{started_at}", sanitize_id(track)));
    if state.records.iter().any(|record| record.id == id) {
        bail!("agent id already exists: {id}");
    }

    let state_dir = state_dir()?;
    fs::create_dir_all(state_dir.join("logs"))?;
    let stdout_log_path = log_path(&id, "out")?;
    let launch = prepare_terminal_launch(
        &id,
        track,
        started_at,
        requested_cwd.as_deref(),
        command,
    )?;
    let backend = selected_terminal_backend()?;
    let record = match backend {
        TerminalBackend::Tmux => {
            start_tmux_terminal_agent(&id, track, started_at, &launch, &stdout_log_path)?
        }
        TerminalBackend::Zellij => start_zellij_terminal_agent(&id, track, started_at, &launch)?,
    };
    state::insert_agent(&state::AgentRow {
        id: record.id.clone(),
        track: record.track.clone(),
        pid: record.pid,
        started_at: record.started_at,
        command: record.command.clone(),
        cwd: record.cwd.clone(),
        stdout_log_path: Some(stdout_log_path),
        stderr_log_path: None,
    })?;
    assign_terminal_display_name(&record)?;
    crate::record_agent_task(&record)?;
    Ok(record)
}

fn selected_terminal_backend() -> Result<TerminalBackend> {
    match env::var("QCOLD_TERMINAL_BACKEND") {
        Ok(value) if value.eq_ignore_ascii_case("zellij") => Ok(TerminalBackend::Zellij),
        Ok(value) if value.trim().is_empty() || value.eq_ignore_ascii_case("tmux") => {
            Ok(TerminalBackend::Tmux)
        }
        Ok(value) => bail!("unsupported QCOLD_TERMINAL_BACKEND={value}; use tmux or zellij"),
        Err(_) => Ok(TerminalBackend::Tmux),
    }
}

fn prepare_launch(
    id: &str,
    track: &str,
    started_at: u64,
    requested_cwd: Option<&Path>,
    command: &[String],
) -> Result<Launch> {
    let command_text = shell_join(command);
    let context = prepare_launch_context(id, track, started_at, requested_cwd, &command_text)?;
    Ok(Launch {
        command: command.to_vec(),
        cwd: context.cwd,
        qcold_repo_root: context.qcold_repo_root,
        qcold_agent_worktree: context.qcold_agent_worktree,
    })
}

fn prepare_terminal_launch(
    id: &str,
    track: &str,
    started_at: u64,
    requested_cwd: Option<&Path>,
    command: &str,
) -> Result<TerminalLaunch> {
    let context = prepare_launch_context(id, track, started_at, requested_cwd, command)?;
    Ok(TerminalLaunch {
        command: command.to_string(),
        cwd: context.cwd,
        qcold_repo_root: context.qcold_repo_root,
        qcold_agent_worktree: context.qcold_agent_worktree,
    })
}

fn prepare_launch_context(
    id: &str,
    track: &str,
    started_at: u64,
    requested_cwd: Option<&Path>,
    command: &str,
) -> Result<LaunchContext> {
    let codex_like = command_contains_codex_agent(command);
    let cwd = if let Some(cwd) = requested_cwd {
        canonical_dir(cwd)?
    } else if codex_like {
        resolve_codex_launch_cwd()?
    } else {
        env::current_dir().context("failed to read current directory")?
    };
    if !should_open_managed_worktree(codex_like, &cwd) {
        return Ok(LaunchContext {
            qcold_repo_root: managed_task_root_for(&cwd),
            qcold_agent_worktree: None,
            cwd,
        });
    }

    open_agent_worktree(id, track, started_at, &cwd)
}

fn resolve_codex_launch_cwd() -> Result<PathBuf> {
    let current = env::current_dir().context("failed to read current directory")?;
    if managed_task_root_for(&current).is_some() {
        return Ok(current);
    }

    let Ok(active_root) = crate::repository::current_or_active_root() else {
        return Ok(current);
    };
    if current.starts_with(&active_root) {
        Ok(current)
    } else {
        Ok(active_root)
    }
}

fn canonical_dir(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    if !path.is_dir() {
        bail!("agent cwd is not a directory: {}", path.display());
    }
    Ok(path)
}

fn command_contains_codex_agent(command: &str) -> bool {
    shell_words(command)
        .iter()
        .filter_map(|word| Path::new(word).file_name().and_then(|name| name.to_str()))
        .any(is_codex_agent_command)
}

fn should_open_managed_worktree(codex_like: bool, cwd: &Path) -> bool {
    codex_like && agent_managed_worktree_enabled() && managed_task_root_for(cwd).is_none()
}

fn agent_managed_worktree_enabled() -> bool {
    env::var("QCOLD_AGENT_MANAGED_WORKTREE")
        .map(|value| !matches!(value.as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(true)
}

fn is_codex_agent_command(name: &str) -> bool {
    matches!(name, "c1" | "cc1" | "c2" | "cc2" | "codex")
        || name
            .strip_prefix("codex")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
}

fn command_path(command: &str) -> Option<PathBuf> {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return executable_file(path).then(|| path.to_path_buf());
    }
    env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(command))
        .find(|candidate| executable_file(candidate))
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn discover_numbered_codex_commands() -> Vec<String> {
    let mut commands = HashSet::new();
    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(ToString::to_string) else {
                    continue;
                };
                if name
                    .strip_prefix("codex")
                    .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
                    && executable_file(&entry.path())
                {
                    commands.insert(name);
                }
            }
        }
    }
    commands.into_iter().collect()
}

fn agent_command_sort_key(command: &str) -> (u8, String) {
    let rank = match command {
        "c1" => 0,
        "cc1" => 1,
        "c2" => 2,
        "cc2" => 3,
        "codex" => 4,
        _ => 5,
    };
    (rank, command.to_string())
}

fn agent_account_key(command: &str) -> String {
    if matches!(command, "c1" | "cc1") {
        return "1".to_string();
    }
    if matches!(command, "c2" | "cc2") {
        return "2".to_string();
    }
    if let Some(suffix) = command.strip_prefix("codex") {
        if !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
            return suffix.to_string();
        }
    }
    "default".to_string()
}

fn status_probe_command(command: &str) -> String {
    match command {
        "cc1" => "c1".to_string(),
        "cc2" => "c2".to_string(),
        _ => command.to_string(),
    }
}

fn managed_task_root_for(cwd: &Path) -> Option<PathBuf> {
    git_root_for(cwd)
        .ok()
        .filter(|root| root.join(".task/task.env").is_file())
}

fn git_root_for(cwd: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .with_context(|| format!("failed to inspect git root for {}", cwd.display()))?;
    if !output.status.success() {
        bail!("not a git worktree: {}", cwd.display());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

fn open_agent_worktree(
    id: &str,
    track: &str,
    started_at: u64,
    requested_cwd: &Path,
) -> Result<LaunchContext> {
    let primary_root = git_root_for(requested_cwd)?;
    let relative_cwd = requested_cwd.strip_prefix(&primary_root).unwrap_or(Path::new(""));
    let agent_slug = agent_worktree_slug(id, track, started_at);
    let worktree = agent_worktree_path(&primary_root, &agent_slug)?;
    if worktree.exists() {
        bail!("agent worktree already exists: {}", worktree.display());
    }
    fs::create_dir_all(
        worktree
            .parent()
            .context("agent worktree has no parent")?,
    )?;
    let worktree_arg = worktree.display().to_string();
    let status = Command::new("git")
        .current_dir(&primary_root)
        .args(["worktree", "add", "--detach", &worktree_arg, "HEAD"])
        .status()
        .with_context(|| format!("failed to create agent worktree {agent_slug}"))?;
    if !status.success() {
        bail!("failed to create agent worktree {agent_slug}: {status}");
    }
    ensure_worktree_submodules(&worktree)?;
    let cwd = worktree.join(relative_cwd);
    let cwd = if cwd.is_dir() {
        cwd
    } else {
        worktree.clone()
    };
    Ok(LaunchContext {
        cwd,
        qcold_repo_root: Some(primary_root),
        qcold_agent_worktree: Some(worktree),
    })
}

fn ensure_worktree_submodules(worktree: &Path) -> Result<()> {
    if !worktree.join(".gitmodules").is_file() {
        return Ok(());
    }
    let output = Command::new("git")
        .current_dir(worktree)
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
            "--recursive",
        ])
        .output()
        .with_context(|| format!("failed to initialize submodules in {}", worktree.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "failed to initialize submodules in {}: {}\n{}",
        worktree.display(),
        output.status,
        format_command_output(&stdout, &stderr)
    );
}

fn agent_worktree_slug(id: &str, track: &str, started_at: u64) -> String {
    let id = sanitize_id(id);
    let track = sanitize_id(track);
    let base = if id.is_empty() {
        format!("agent-{track}-{started_at}")
    } else {
        format!("agent-{id}")
    };
    if base == "agent--" || base == "agent-" {
        format!("agent-{started_at}")
    } else {
        base
    }
}

fn agent_worktree_path(primary_root: &Path, agent_slug: &str) -> Result<PathBuf> {
    Ok(primary_root
        .parent()
        .context("repository root has no parent")?
        .join("WT")
        .join(primary_root.file_name().context("repository root has no name")?)
        .join("agents")
        .join(agent_slug))
}

fn format_command_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim(), stderr.trim()) {
        ("", "") => "no output".to_string(),
        (stdout, "") => stdout.to_string(),
        ("", stderr) => stderr.to_string(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    }
}

fn start_tmux_terminal_agent(
    id: &str,
    track: &str,
    started_at: u64,
    launch: &TerminalLaunch,
    stdout_log_path: &Path,
) -> Result<AgentRecord> {
    ensure_tmux_available()?;
    let session = format!("qcold-{id}");
    let target = format!("{session}:0.0");
    let env_prefix = terminal_qcold_env_prefix(
        launch.qcold_repo_root.as_deref(),
        launch.qcold_agent_worktree.as_deref(),
    );
    let wrapped = format!(
        "{env_prefix}{}; status=$?; printf '\\n[Q-COLD terminal command exited with status %s]\\n' \"$status\"; exit \"$status\"",
        launch.command,
    );
    let delayed = format!("sleep 0.1; exec sh -lc {}", shell_quote(&wrapped));
    let tmux_shell_command = format!("sh -lc {}", shell_quote(&delayed));
    let status = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            &session,
            "-c",
            &launch.cwd.display().to_string(),
            &tmux_shell_command,
        ])
        .status()
        .with_context(|| format!("failed to start tmux session {session}"))?;
    if !status.success() {
        bail!("tmux new-session failed with {status}");
    }
    set_tmux_option(&session, "remain-on-exit", "off")?;
    set_tmux_option(&session, "mouse", "off")?;
    let pipe_command = format!("cat >> {}", shell_quote(&stdout_log_path.display().to_string()));
    let status = Command::new("tmux")
        .args(["pipe-pane", "-o", "-t", &target, &pipe_command])
        .status()
        .with_context(|| format!("failed to pipe tmux pane {target}"))?;
    if !status.success() {
        bail!("tmux pipe-pane failed with {status}");
    }
    let pid = tmux_pane_pid(&target)?;

    let record = AgentRecord {
        id: id.to_string(),
        track: track.to_string(),
        pid,
        started_at,
        command: vec![
            "tmux".to_string(),
            "new-session".to_string(),
            "-s".to_string(),
            session,
            launch.command.clone(),
        ],
        cwd: Some(launch.cwd.clone()),
    };
    Ok(record)
}

fn start_zellij_terminal_agent(
    id: &str,
    track: &str,
    started_at: u64,
    launch: &TerminalLaunch,
) -> Result<AgentRecord> {
    ensure_zellij_available()?;
    let session = format!("qcold-{id}");
    let env_prefix = terminal_qcold_env_prefix(
        launch.qcold_repo_root.as_deref(),
        launch.qcold_agent_worktree.as_deref(),
    );
    let wrapped = format!(
        "{env_prefix}{}; status=$?; printf '\\n[Q-COLD terminal command exited with status %s]\\n' \"$status\"; sleep 0.1; zellij kill-session {} >/dev/null 2>&1 || true; exit \"$status\"",
        launch.command,
        shell_quote(&session)
    );
    let layout_path = state_dir()?.join("logs").join(format!("{id}.zellij.kdl"));
    fs::write(&layout_path, zellij_layout(id, &wrapped)?)
        .with_context(|| format!("failed to write zellij layout {}", layout_path.display()))?;

    let status = Command::new("zellij")
        .current_dir(&launch.cwd)
        .args([
            "attach",
            "--create-background",
            "--forget",
            &session,
            "options",
            "--default-layout",
            &layout_path.display().to_string(),
            "--mouse-mode",
            "false",
            "--pane-frames",
            "false",
            "--show-release-notes",
            "false",
            "--show-startup-tips",
            "false",
        ])
        .status()
        .with_context(|| format!("failed to create zellij session {session}"))?;
    if !status.success() {
        bail!("zellij attach --create-background failed with {status}");
    }
    let pane = zellij_first_terminal_pane(&session)?;
    let _ = Command::new("zellij")
        .args(["--session", &session, "action", "focus-pane-id", &pane])
        .status();
    let pid = zellij_session_pid(&session)?;

    Ok(AgentRecord {
        id: id.to_string(),
        track: track.to_string(),
        pid,
        started_at,
        command: vec![
            "zellij".to_string(),
            "--session".to_string(),
            session,
            "pane".to_string(),
            pane,
            launch.command.clone(),
        ],
        cwd: Some(launch.cwd.clone()),
    })
}

fn zellij_layout(id: &str, wrapped: &str) -> Result<String> {
    Ok(format!(
        "layout {{\n    pane name={} command=\"sh\" close_on_exit=true {{\n        args \"-lc\" {}\n    }}\n}}\n",
        kdl_quote(id)?,
        kdl_quote(wrapped)?
    ))
}

fn kdl_quote(value: &str) -> Result<String> {
    serde_json::to_string(value).context("failed to quote zellij layout string")
}

fn zellij_first_terminal_pane(session: &str) -> Result<String> {
    let mut last_error = None;
    for _ in 0..20 {
        match zellij_first_terminal_pane_once(session) {
            Ok(pane) => return Ok(pane),
            Err(err) => last_error = Some(err),
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(last_error
        .unwrap_or_else(|| anyhow::anyhow!("zellij session {session} has no terminal pane")))
}

fn apply_qcold_launch_env(
    command: &mut Command,
    root: Option<&Path>,
    agent_worktree: Option<&Path>,
) {
    if let Some(root) = root {
        command.env("QCOLD_REPO_ROOT", root);
    }
    if let Some(agent_worktree) = agent_worktree {
        command.env("QCOLD_AGENT_WORKTREE", agent_worktree);
    }
}

fn terminal_qcold_env_prefix(root: Option<&Path>, agent_worktree: Option<&Path>) -> String {
    let mut prefix = String::new();
    if let Some(root) = root {
        prefix.push_str(&format!(
            "export QCOLD_REPO_ROOT={}; ",
            shell_quote(&root.display().to_string())
        ));
    }
    if let Some(agent_worktree) = agent_worktree {
        prefix.push_str(&format!(
            "export QCOLD_AGENT_WORKTREE={}; ",
            shell_quote(&agent_worktree.display().to_string())
        ));
    }
    prefix
}

fn zellij_first_terminal_pane_once(session: &str) -> Result<String> {
    let output = Command::new("zellij")
        .args(["--session", session, "action", "list-panes"])
        .output()
        .with_context(|| format!("failed to list zellij panes for session {session}"))?;
    if !output.status.success() {
        bail!("zellij action list-panes failed with {}", output.status);
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .find_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            (fields.len() >= 2 && fields[1] == "terminal").then(|| fields[0].to_string())
        })
        .with_context(|| format!("zellij session {session} has no terminal pane"))
}

fn set_tmux_option(session: &str, name: &str, value: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["set-option", "-t", session, name, value])
        .status()
        .with_context(|| format!("failed to configure tmux session {session}"))?;
    if !status.success() {
        bail!("tmux set-option {name} failed with {status}");
    }
    Ok(())
}

fn ensure_zellij_available() -> Result<()> {
    let status = Command::new("zellij")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("zellij is required for QCOLD_TERMINAL_BACKEND=zellij")?;
    if !status.success() {
        bail!("zellij is required for QCOLD_TERMINAL_BACKEND=zellij");
    }
    Ok(())
}

fn zellij_session_pid(session: &str) -> Result<u32> {
    let marker = format!("/zellij/contract_version_1/{session}");
    let entries = fs::read_dir("/proc").context("failed to inspect /proc for zellij session")?;
    for entry in entries.filter_map(Result::ok) {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args = cmdline
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part));
        if args.into_iter().any(|arg| arg.contains(&marker)) {
            return Ok(pid);
        }
    }
    bail!("failed to locate zellij server process for session {session}");
}

fn ensure_tmux_available() -> Result<()> {
    let status = Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("tmux is required for attachable terminal agents")?;
    if !status.success() {
        bail!("tmux is required for attachable terminal agents");
    }
    Ok(())
}

fn tmux_pane_pid(target: &str) -> Result<u32> {
    let output = Command::new("tmux")
        .args(["display-message", "-p", "-t", target, "#{pane_pid}"])
        .output()
        .with_context(|| format!("failed to read tmux pane pid for {target}"))?;
    if !output.status.success() {
        bail!("tmux display-message failed with {}", output.status);
    }
    let value = String::from_utf8_lossy(&output.stdout);
    value
        .trim()
        .parse()
        .with_context(|| format!("invalid tmux pane pid for {target}: {value}"))
}

fn attach_terminal(record: &AgentRecord) -> Result<()> {
    let target = terminal_target(record).context("agent was not started in a terminal session")?;
    let (program, args, session) = match target {
        TerminalTarget::Tmux { session } => (
            "tmux",
            vec!["attach-session".to_string(), "-t".to_string(), session.clone()],
            session,
        ),
        TerminalTarget::Zellij { session, .. } => {
            ("zellij", vec!["attach".to_string(), session.clone()], session)
        }
    };
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to attach terminal session {session}"))?;
    if !status.success() {
        bail!("terminal attach failed with {status}");
    }
    Ok(())
}

fn terminal_target(record: &AgentRecord) -> Option<TerminalTarget> {
    match record.command.as_slice() {
        [tmux, new_session, flag, session, ..]
            if tmux == "tmux" && new_session == "new-session" && flag == "-s" =>
        {
            Some(TerminalTarget::Tmux {
                session: session.clone(),
            })
        }
        [zellij, session_flag, session, pane_marker, pane, ..]
            if zellij == "zellij" && session_flag == "--session" && pane_marker == "pane" =>
        {
            Some(TerminalTarget::Zellij {
                session: session.clone(),
                pane: pane.clone(),
            })
        }
        _ => None,
    }
}

fn terminal_command_from_record(command: &[String]) -> String {
    match command {
        [tmux, new_session, flag, _session, wrapped, ..]
            if tmux == "tmux" && new_session == "new-session" && flag == "-s" =>
        {
            wrapped.clone()
        }
        [zellij, session_flag, _session, pane_marker, _pane, wrapped, ..]
            if zellij == "zellij" && session_flag == "--session" && pane_marker == "pane" =>
        {
            wrapped.clone()
        }
        _ => command.join(" "),
    }
}

fn render_record(
    record: &AgentRecord,
    metadata: &HashMap<String, state::TerminalMetadataRow>,
) -> String {
    render_record_with_state(record, metadata, process_state(record.pid))
}

fn render_record_with_state(
    record: &AgentRecord,
    metadata: &HashMap<String, state::TerminalMetadataRow>,
    state: &str,
) -> String {
    let mut line = format!(
        "agent\t{}\ttrack={}\tpid={}\tstate={}\tstarted_at={}\tcmd={}",
        record.id,
        record.track,
        record.pid,
        state,
        record.started_at,
        record.command.join(" ")
    );
    if let Some(cwd) = &record.cwd {
        let _ = write!(line, "\tcwd={}", cwd.display());
    }
    if let Some(name) = terminal_display_name(record, metadata) {
        let _ = write!(line, "\tname={name}");
    }
    if let Some(target) = terminal_target(record) {
        match target {
            TerminalTarget::Tmux { session } => {
                let _ = write!(
                    line,
                    "\tterminal={session}\ttarget={session}:0.0\tattach=tmux attach-session -t {session}"
                );
            }
            TerminalTarget::Zellij { session, pane } => {
                let _ = write!(
                    line,
                    "\tterminal={session}\ttarget=zellij:{session}:{pane}\tattach=zellij attach {session}"
                );
            }
        }
    }
    line
}

fn assign_terminal_display_name(record: &AgentRecord) -> Result<()> {
    let Some(target) = terminal_target_key(record) else {
        return Ok(());
    };
    let metadata = terminal_metadata_by_target()?;
    if metadata
        .get(&target)
        .and_then(|metadata| metadata.name.as_deref())
        .is_some_and(|name| !name.trim().is_empty())
    {
        return Ok(());
    }
    let used = used_terminal_display_names(&metadata)?;
    let name = choose_agent_display_name(&record.id, &used);
    state::save_terminal_metadata(&target, Some(&name), None)
}

fn terminal_display_name<'a>(
    record: &AgentRecord,
    metadata: &'a HashMap<String, state::TerminalMetadataRow>,
) -> Option<&'a str> {
    let target = terminal_target_key(record)?;
    metadata
        .get(&target)
        .and_then(|metadata| metadata.name.as_deref())
        .filter(|name| !name.trim().is_empty())
}

fn terminal_metadata_by_target() -> Result<HashMap<String, state::TerminalMetadataRow>> {
    Ok(state::load_terminal_metadata()?
        .into_iter()
        .map(|metadata| (metadata.target.clone(), metadata))
        .collect())
}

fn used_terminal_display_names(
    metadata: &HashMap<String, state::TerminalMetadataRow>,
) -> Result<HashSet<String>> {
    Ok(AgentState::load()?
        .records
        .into_iter()
        .filter(|record| process_state(record.pid) == "running")
        .filter_map(|record| terminal_display_name(&record, metadata).map(normalize_display_name))
        .collect())
}

fn terminal_target_key(record: &AgentRecord) -> Option<String> {
    match terminal_target(record)? {
        TerminalTarget::Tmux { session } => Some(format!("{session}:0.0")),
        TerminalTarget::Zellij { session, pane } => Some(format!("zellij:{session}:{pane}")),
    }
}

fn choose_agent_display_name(id: &str, used: &HashSet<String>) -> String {
    let start = stable_name_offset(id);
    for round in 0..100 {
        for offset in 0..AGENT_DISPLAY_NAMES.len() {
            let name = AGENT_DISPLAY_NAMES[(start + offset) % AGENT_DISPLAY_NAMES.len()];
            let candidate = if round == 0 {
                name.to_string()
            } else {
                format!("{name} {}", round + 1)
            };
            if !used.contains(&normalize_display_name(&candidate)) {
                return candidate;
            }
        }
    }
    format!("Agent {}", short_agent_id(id))
}

fn stable_name_offset(value: &str) -> usize {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    (hash as usize) % AGENT_DISPLAY_NAMES.len()
}

fn normalize_display_name(value: impl AsRef<str>) -> String {
    value.as_ref().trim().to_ascii_lowercase()
}

fn short_agent_id(id: &str) -> String {
    id.chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn process_state(pid: u32) -> &'static str {
    if PathBuf::from(format!("/proc/{pid}")).exists() {
        "running"
    } else {
        "exited"
    }
}

fn log_path(id: &str, stream: &str) -> Result<PathBuf> {
    Ok(state_dir()?.join("logs").join(format!("{id}.{stream}.log")))
}

fn log_file(path: &PathBuf) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open agent log {}", path.display()))
}

fn sanitize_id(value: &str) -> String {
    let id: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    id.trim_matches('-').to_string()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escape = false;
    for ch in command.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn unix_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRecord {
    pub(crate) id: String,
    pub(crate) track: String,
    pub(crate) pid: u32,
    pub(crate) started_at: u64,
    pub(crate) command: Vec<String>,
    pub(crate) cwd: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalAgentContext {
    pub id: String,
    pub track: String,
    pub session: String,
    pub pane: String,
    pub target: String,
    pub started_at: u64,
    pub command: String,
}

struct AgentState {
    records: Vec<AgentRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapshotScope {
    All,
    RunningOnly,
}

impl AgentState {
    fn load() -> Result<Self> {
        let records = state::load_agents(&registry_path()?)?
            .into_iter()
            .map(|row| AgentRecord {
                id: row.id,
                track: row.track,
                pid: row.pid,
                started_at: row.started_at,
                command: row.command,
                cwd: row.cwd,
            })
            .collect();
        Ok(Self { records })
    }
}

#[cfg(test)]
fn parse_record(line: &str) -> Result<AgentRecord> {
    let fields = line.split('\t').collect::<Vec<_>>();
    if fields.len() != 5 {
        bail!("invalid agent registry line: {line}");
    }
    Ok(AgentRecord {
        id: unescape_field(fields[0]),
        track: unescape_field(fields[1]),
        pid: fields[2]
            .parse()
            .with_context(|| format!("invalid agent pid: {}", fields[2]))?,
        started_at: fields[3]
            .parse()
            .with_context(|| format!("invalid agent start time: {}", fields[3]))?,
        command: unescape_field(fields[4])
            .split('\u{1f}')
            .map(ToString::to_string)
            .collect(),
        cwd: None,
    })
}

#[cfg(test)]
fn serialize_record(record: &AgentRecord) -> String {
    [
        escape_field(&record.id),
        escape_field(&record.track),
        record.pid.to_string(),
        record.started_at.to_string(),
        escape_field(&record.command.join("\u{1f}")),
    ]
    .join("\t")
}

#[cfg(test)]
fn escape_field(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\t', "\\t")
}

#[cfg(test)]
fn unescape_field(value: &str) -> String {
    value.replace("\\t", "\t").replace("\\\\", "\\")
}

pub(crate) fn registry_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("agents.tsv"))
}

fn state_dir() -> Result<PathBuf> {
    if let Ok(path) = env::var("QCOLD_STATE_DIR") {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    let home = env::var("HOME").context("HOME is required when QCOLD_STATE_DIR is unset")?;
    Ok(PathBuf::from(home).join(".local/state/qcold"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use tempfile::tempdir;

    fn git_ok(cwd: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .current_dir(cwd)
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git command failed in {}: {:?}",
            cwd.display(),
            args
        );
    }

    fn seed_git_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        git_ok(path, &["init"]);
        git_ok(path, &["config", "user.name", "tester"]);
        git_ok(path, &["config", "user.email", "tester@example.com"]);
        fs::write(path.join("README.md"), "seed\n").unwrap();
        git_ok(path, &["add", "README.md"]);
        git_ok(path, &["commit", "-m", "seed"]);
    }

    #[test]
    fn records_round_trip() {
        let record = AgentRecord {
            id: "agent-1".to_string(),
            track: "track".to_string(),
            pid: 123,
            started_at: 456,
            command: vec!["sh".to_string(), "-c".to_string(), "echo hi".to_string()],
            cwd: None,
        };
        assert_eq!(parse_record(&serialize_record(&record)).unwrap(), record);
    }

    #[test]
    fn codex_wrappers_use_agent_worktree_launch_cwd() {
        let temp = tempdir().unwrap();
        assert!(command_contains_codex_agent("cc1 \"inspect submodules\""));
        assert!(command_contains_codex_agent(
            "/home/qqrm/.local/bin/cc2 \"fix context reset\""
        ));
        assert!(command_contains_codex_agent("codex3 exec \"audit\""));
        assert!(!command_contains_codex_agent("printf ok"));
        assert!(should_open_managed_worktree(true, temp.path()));
        assert!(!should_open_managed_worktree(false, temp.path()));
    }

    #[test]
    fn agent_worktree_paths_are_separate_from_task_inventory() {
        let temp = tempdir().unwrap();
        let primary = temp.path().join("repo");
        fs::create_dir_all(&primary).unwrap();
        assert_eq!(
            agent_worktree_path(&primary, "agent-c1-123")
                .unwrap()
                .strip_prefix(temp.path())
                .unwrap(),
            Path::new("WT/repo/agents/agent-c1-123")
        );
    }

    #[test]
    fn agent_worktree_creation_does_not_create_task_env() {
        let temp = tempdir().unwrap();
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);

        let context = open_agent_worktree("c1-123", "c1", 123, &primary).unwrap();
        assert_eq!(context.qcold_repo_root.as_deref(), Some(primary.as_path()));
        assert_eq!(
            context.qcold_agent_worktree.as_deref(),
            Some(context.cwd.as_path())
        );
        assert!(context
            .cwd
            .strip_prefix(temp.path().join("WT/repo/agents"))
            .is_ok());
        assert!(!context.cwd.join(".task/task.env").exists());
    }

    #[test]
    fn agent_worktree_initializes_local_file_submodules() {
        let temp = tempdir().unwrap();
        let submodule = temp.path().join("json11-src");
        seed_git_repo(&submodule);

        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let submodule_arg = submodule.display().to_string();
        git_ok(
            &primary,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                &submodule_arg,
                "json11",
            ],
        );
        git_ok(&primary, &["commit", "-m", "add json11 submodule"]);

        let context = open_agent_worktree("c1-submodule", "c1", 456, &primary).unwrap();
        assert!(context.cwd.join("json11/README.md").is_file());
    }

    #[test]
    fn terminal_env_prefix_exports_agent_worktree() {
        let prefix = terminal_qcold_env_prefix(
            Some(Path::new("/workspace/primary")),
            Some(Path::new("/workspace/WT/repo/agents/c1")),
        );
        assert!(prefix.contains("export QCOLD_REPO_ROOT='/workspace/primary';"));
        assert!(prefix.contains("export QCOLD_AGENT_WORKTREE='/workspace/WT/repo/agents/c1';"));
    }

    #[test]
    fn agent_display_name_uses_unused_pool_name() {
        let mut used = HashSet::new();
        for name in AGENT_DISPLAY_NAMES {
            used.insert(normalize_display_name(name));
        }

        let name = choose_agent_display_name("c1-1234", &used);
        assert!(name.ends_with(" 2"));
        assert!(!used.contains(&normalize_display_name(&name)));
    }

    #[test]
    fn snapshot_line_includes_terminal_display_name() {
        let record = AgentRecord {
            id: "c1-1234".to_string(),
            track: "c1".to_string(),
            pid: std::process::id(),
            started_at: 456,
            command: vec![
                "tmux".to_string(),
                "new-session".to_string(),
                "-s".to_string(),
                "qcold-c1-1234".to_string(),
                "c1 \"inspect\"".to_string(),
            ],
            cwd: None,
        };
        let mut metadata = HashMap::new();
        metadata.insert(
            "qcold-c1-1234:0.0".to_string(),
            state::TerminalMetadataRow {
                target: "qcold-c1-1234:0.0".to_string(),
                name: Some("Socrates".to_string()),
                scope: None,
                updated_at: 123,
            },
        );

        assert!(render_record(&record, &metadata).contains("\tname=Socrates\t"));
    }

    #[test]
    fn running_snapshot_omits_exited_agent_records() {
        let records = vec![
            AgentRecord {
                id: "active-agent".to_string(),
                track: "unit".to_string(),
                pid: std::process::id(),
                started_at: 100,
                command: vec!["sleep".to_string(), "10".to_string()],
                cwd: None,
            },
            AgentRecord {
                id: "exited-agent".to_string(),
                track: "unit".to_string(),
                pid: u32::MAX,
                started_at: 101,
                command: vec!["printf".to_string(), "done".to_string()],
                cwd: None,
            },
        ];
        let metadata = HashMap::new();

        let snapshot =
            render_snapshot_with_metadata(&records, SnapshotScope::RunningOnly, &metadata);

        assert!(snapshot.starts_with("agents\tcount=1\n"));
        assert!(snapshot.contains("agent\tactive-agent\t"));
        assert!(!snapshot.contains("exited-agent"));
    }

    #[test]
    fn start_shell_agent_records_process() {
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path());
        let record = start_agent(
            None,
            "unit".to_string(),
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "sleep 1".to_string(),
            ],
            Some(temp.path().to_path_buf()),
        )
        .unwrap();
        assert!(record.id.starts_with("unit-"));
        let snapshot = snapshot().unwrap();
        assert!(snapshot.contains("agent\tunit-"));
        env::remove_var("QCOLD_STATE_DIR");
    }
}
