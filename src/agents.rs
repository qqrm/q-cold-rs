use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::output_guard::{prepare_output_guard_launch, OutputGuardLaunch};
#[cfg(test)]
use crate::output_guard::{
    output_guard_commands, parse_output_guard_commands, prepare_output_guard_launch_with_paths,
    write_output_guard_wrapper, GuardedCommand,
};
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
];
const DEFAULT_AGENT_STALE_TTL_HOURS: u64 = 2;

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

#[derive(Clone, Copy, PartialEq, Eq)]
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
    output_guard: Option<OutputGuardLaunch>,
}

struct TerminalLaunch {
    command: String,
    cwd: PathBuf,
    qcold_repo_root: Option<PathBuf>,
    qcold_agent_worktree: Option<PathBuf>,
    output_guard: Option<OutputGuardLaunch>,
    zellij_pane_name: Option<String>,
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
    #[command(about = "Attach to a tracked terminal agent")]
    Attach(AttachArgs),
    #[command(about = "List tracked agent processes")]
    List,
    #[command(about = "Prune stale terminal agents and ad-hoc task records")]
    PruneStale(PruneStaleArgs),
}

#[derive(Args)]
struct AttachArgs {
    #[arg(help = "Agent id, terminal target, session name, or terminal display name")]
    selector: String,
}

#[derive(Args)]
struct StartArgs {
    #[arg(long)]
    id: Option<String>,
    #[arg(long, help = "Set the zellij pane name for a terminal agent")]
    name: Option<String>,
    #[arg(long)]
    track: String,
    #[arg(long, help = "Directory used as the agent launch context")]
    cwd: Option<PathBuf>,
    #[arg(long, help = "Run the agent in an attachable terminal session")]
    terminal: bool,
    #[arg(long, help = "Attach to the terminal after starting the agent")]
    attach: bool,
    #[arg(required = true, trailing_var_arg = true)]
    command: Vec<String>,
}

#[derive(Args)]
struct PruneStaleArgs {
    #[arg(long, help = "Age threshold in hours; defaults to QCOLD_AGENT_STALE_TTL_HOURS or 2")]
    max_age_hours: Option<u64>,
    #[arg(long, help = "Also terminate terminal agents that still have attached clients")]
    include_attached: bool,
    #[arg(long, help = "Show what would be pruned without mutating state")]
    dry_run: bool,
    #[arg(long, help = "Print one row per pruned or candidate agent")]
    verbose: bool,
}

pub fn run(args: AgentArgs) -> Result<u8> {
    match args.command {
        AgentCommand::Start(args) => {
            let record = if args.terminal || args.attach {
                if args.attach {
                    if let Some(name) = clean_zellij_pane_name(args.name.as_deref())? {
                        if let Some(record) = running_named_terminal_record(&args.track, &name)? {
                            println!("{}", snapshot_line(&record));
                            attach_terminal(&record)?;
                            return Ok(0);
                        }
                    }
                }
                start_terminal_agent(
                    args.id,
                    &args.track,
                    &shell_join(&args.command),
                    args.cwd.as_deref(),
                    args.name.as_deref(),
                )?
            } else {
                if args.name.is_some() {
                    bail!("--name requires --terminal or --attach");
                }
                start_agent(args.id, &args.track, &args.command, args.cwd.as_deref())?
            };
            println!("{}", snapshot_line(&record));
            if args.attach {
                attach_terminal(&record)?;
            }
        }
        AgentCommand::Attach(args) => attach_tracked_terminal(&args.selector)?,
        AgentCommand::List => print!("{}", snapshot()?),
        AgentCommand::PruneStale(args) => {
            let max_age_hours = args.max_age_hours.map_or_else(agent_stale_ttl_hours, Ok)?;
            let summary = prune_stale_agents(max_age_hours, args.include_attached, args.dry_run)?;
            println!("{}", summary.render());
            if args.verbose {
                for event in summary.events {
                    println!("{}", event.render());
                }
            }
        }
    }
    Ok(0)
}

pub fn snapshot() -> Result<String> {
    let _ = crate::sync_codex_task_records();
    prune_stale_agents_best_effort();
    let state = AgentState::load()?;
    Ok(render_snapshot(&state.records, SnapshotScope::All))
}

pub fn running_snapshot() -> Result<String> {
    let _ = crate::sync_codex_task_records();
    prune_stale_agents_best_effort();
    let state = AgentState::load()?;
    Ok(render_snapshot(&state.records, SnapshotScope::RunningOnly))
}

pub fn available_agent_commands() -> Vec<AvailableAgentCommand> {
    let path_env = env::var_os("PATH");
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    available_agent_commands_from(path_env.as_deref(), &home)
}

fn available_agent_commands_from(path_env: Option<&OsStr>, home: &Path) -> Vec<AvailableAgentCommand> {
    let mut commands = Vec::new();
    let mut seen = HashSet::new();
    for (command, label, invocation) in KNOWN_AGENT_COMMANDS {
        let account = agent_account_key(command);
        if !agent_account_authenticated_in_home(&account, home) {
            continue;
        }
        if let Some(path) = command_path_in_paths(command, path_env) {
            seen.insert((*command).to_string());
            commands.push(AvailableAgentCommand {
                command: (*command).to_string(),
                label: (*label).to_string(),
                invocation: invocation.as_str(),
                path: path.display().to_string(),
                account,
                status_command: status_probe_command(command),
            });
        }
    }
    for command in discover_numbered_codex_commands_in_paths(path_env) {
        if !seen.insert(command.clone()) {
            continue;
        }
        let account = agent_account_key(&command);
        if !agent_account_authenticated_in_home(&account, home) {
            continue;
        }
        if let Some(path) = command_path_in_paths(&command, path_env) {
            commands.push(AvailableAgentCommand {
                label: format!("Codex account {}", command.trim_start_matches("codex")),
                account,
                status_command: status_probe_command(&command),
                command,
                invocation: AgentInvocation::Exec.as_str(),
                path: path.display().to_string(),
            });
        }
    }
    commands.sort_by_key(|left| agent_command_sort_key(&left.command));
    commands
}

pub(crate) fn agent_auth_file(account: &str) -> PathBuf {
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    agent_auth_file_in_home(account, &home)
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
    prune_stale_agents_best_effort();
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

pub fn terminate_agent(id: &str) -> Result<bool> {
    let Some(record) = AgentState::load()?.records.into_iter().find(|record| record.id == id) else {
        return Ok(false);
    };
    if let Some(target) = terminal_target(&record) {
        terminate_terminal_target(&target)?;
        return Ok(true);
    }
    terminate_process(record.pid)?;
    Ok(true)
}

include!("agents/stale_prune.rs");

pub fn start_shell_agent(track: &str, command: &str) -> Result<AgentRecord> {
    if command.trim().is_empty() {
        bail!("agent command is empty");
    }
    let cwd = None;
    let command = vec!["sh".to_string(), "-c".to_string(), command.to_string()];
    start_agent(
        None,
        track,
        &command,
        cwd,
    )
}

pub fn start_terminal_shell_agent_with_id(
    id: Option<String>,
    track: &str,
    command: &str,
) -> Result<AgentRecord> {
    if command.trim().is_empty() {
        bail!("agent command is empty");
    }
    start_terminal_agent(id, track, command, None, None)
}

pub fn start_terminal_shell_agent_with_id_in_cwd(
    id: Option<String>,
    track: &str,
    command: &str,
    cwd: &Path,
) -> Result<AgentRecord> {
    if command.trim().is_empty() {
        bail!("agent command is empty");
    }
    start_terminal_agent(id, track, command, Some(cwd), None)
}

fn start_agent(
    id: Option<String>,
    track: &str,
    command: &[String],
    requested_cwd: Option<&Path>,
) -> Result<AgentRecord> {
    if track.trim().is_empty() {
        bail!("agent track is empty");
    }
    if command.is_empty() {
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
    let stderr_log_path = log_path(&id, "err")?;
    let stdout = log_file(&stdout_log_path)?;
    let stderr = log_file(&stderr_log_path)?;
    let launch = prepare_launch(&id, track, started_at, requested_cwd, command)?;
    let mut process = Command::new(&launch.command[0]);
    process.args(&launch.command[1..]);
    process.current_dir(&launch.cwd);
    apply_qcold_launch_env(
        &mut process,
        launch.qcold_repo_root.as_deref(),
        launch.qcold_agent_worktree.as_deref(),
        launch.output_guard.as_ref(),
    );
    let child = process
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("failed to start agent command: {}", command.join(" ")))?;

    let record = AgentRecord {
        id,
        track: track.to_string(),
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
    requested_cwd: Option<&Path>,
    zellij_pane_name: Option<&str>,
) -> Result<AgentRecord> {
    if track.trim().is_empty() {
        bail!("agent track is empty");
    }
    if command.trim().is_empty() {
        bail!("agent command is empty");
    }
    let backend = selected_terminal_backend()?;
    let zellij_pane_name = clean_zellij_pane_name(zellij_pane_name)?;
    if zellij_pane_name.is_some() && backend != TerminalBackend::Zellij {
        bail!("--name is only supported with QCOLD_TERMINAL_BACKEND=zellij");
    }
    let state = AgentState::load()?;
    let started_at = unix_now()?;
    let id = id.unwrap_or_else(|| format!("{}-{started_at}", sanitize_id(track)));
    if state.records.iter().any(|record| record.id == id) {
        bail!("agent id already exists: {id}");
    }
    let named_resume =
        named_codex_resume_launch(track, command, requested_cwd, zellij_pane_name.as_deref())?;
    let launch_command = named_resume
        .as_ref()
        .map_or(command, |resume| resume.command.as_str());
    let launch_cwd = named_resume
        .as_ref()
        .map(|resume| resume.cwd.as_path())
        .or(requested_cwd);

    let state_dir = state_dir()?;
    fs::create_dir_all(state_dir.join("logs"))?;
    let stdout_log_path = log_path(&id, "out")?;
    let launch = prepare_terminal_launch(
        &id,
        track,
        started_at,
        launch_cwd,
        launch_command,
        zellij_pane_name.clone(),
    )?;
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
    assign_terminal_display_name(&record, zellij_pane_name.as_deref())?;
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

fn running_named_terminal_record(track: &str, requested_name: &str) -> Result<Option<AgentRecord>> {
    let metadata = terminal_metadata_by_target()?;
    let requested_name = normalize_display_name(requested_name);
    let matches = AgentState::load()?
        .records
        .into_iter()
        .filter(|record| record.track == track)
        .filter(|record| process_state(record.pid) == "running")
        .filter(|record| terminal_target(record).is_some())
        .filter(|record| {
            terminal_display_name(record, &metadata)
                .is_some_and(|name| normalize_display_name(name) == requested_name)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [record] => Ok(Some(record.clone())),
        records => bail!(
            "terminal name {requested_name:?} is ambiguous for track {track:?}; matched {}",
            records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
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
    let output_guard = prepare_output_guard_launch(id, started_at)?;
    Ok(Launch {
        command: command.to_vec(),
        cwd: context.cwd,
        qcold_repo_root: context.qcold_repo_root,
        qcold_agent_worktree: context.qcold_agent_worktree,
        output_guard,
    })
}

fn prepare_terminal_launch(
    id: &str,
    track: &str,
    started_at: u64,
    requested_cwd: Option<&Path>,
    command: &str,
    zellij_pane_name: Option<String>,
) -> Result<TerminalLaunch> {
    let context = prepare_launch_context(id, track, started_at, requested_cwd, command)?;
    let output_guard = prepare_output_guard_launch(id, started_at)?;
    Ok(TerminalLaunch {
        command: command.to_string(),
        cwd: context.cwd,
        qcold_repo_root: context.qcold_repo_root,
        qcold_agent_worktree: context.qcold_agent_worktree,
        output_guard,
        zellij_pane_name,
    })
}

fn clean_zellij_pane_name(value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.chars().any(char::is_control) {
        bail!("zellij pane name contains a control character");
    }
    let mut value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        bail!("zellij pane name is empty");
    }
    crate::prompt::truncate_chars(&mut value, 80);
    Ok(Some(value))
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
    if codex_like {
        if let Some(context) = existing_agent_worktree_context(&cwd)? {
            return Ok(context);
        }
    }
    if !should_open_managed_worktree(codex_like, &cwd) {
        return Ok(LaunchContext {
            qcold_repo_root: managed_task_root_for(&cwd),
            qcold_agent_worktree: None,
            cwd,
        });
    }
    if let Some(context) = reusable_codex_agent_context(track, command, requested_cwd, &cwd)? {
        return Ok(context);
    }

    open_agent_worktree(id, track, started_at, &cwd)
}

fn resolve_codex_launch_cwd() -> Result<PathBuf> {
    let current = env::current_dir().context("failed to read current directory")?;
    if managed_task_root_for(&current).is_some() {
        return Ok(current);
    }
    if git_root_for(&current).is_ok() {
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

include!("agents/reuse_context.rs");

fn codex_account_from_command(command: &str) -> Option<String> {
    shell_words(command)
        .iter()
        .filter_map(|word| Path::new(word).file_name().and_then(|name| name.to_str()))
        .find(|name| is_codex_agent_command(name))
        .map(agent_account_key)
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
        .map_or(true, |value| !matches!(value.as_str(), "0" | "false" | "no" | "off"))
}

fn is_codex_agent_command(name: &str) -> bool {
    matches!(name, "c1" | "cc1" | "c2" | "cc2" | "codex")
        || name
            .strip_prefix("codex")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
}

fn command_path_in_paths(command: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return executable_file(path).then(|| path.to_path_buf());
    }
    path_env
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

fn discover_numbered_codex_commands_in_paths(path_env: Option<&OsStr>) -> Vec<String> {
    let mut commands = HashSet::new();
    if let Some(paths) = path_env {
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

fn agent_auth_file_in_home(account: &str, home: &Path) -> PathBuf {
    if account == "default" {
        return home.join(".codex/auth.json");
    }
    home.join(".codex-accounts").join(account).join("auth.json")
}

fn agent_account_authenticated_in_home(account: &str, home: &Path) -> bool {
    agent_auth_file_in_home(account, home).is_file()
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


include!("agents/worktree_terminal.rs");
include!("agents/render_state_tests.rs");
include!("agents/output_guard_tests.rs");
include!("agents/terminal_metadata_tests.rs");
include!("agents/zellij_name_tests.rs");
include!("agents/resume_tests.rs");
include!("agents/available_commands_tests.rs");
