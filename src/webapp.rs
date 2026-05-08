use std::collections::HashMap;
use std::convert::Infallible;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use axum::{
    extract::Json,
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        HeaderMap, StatusCode,
    },
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    routing::{get, post},
    Router,
};
use clap::Args;
use futures_util::stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{agents, history, repository, state, status};

const DAEMON_STARTUP_CHECKS: usize = 10;
const DAEMON_STARTUP_CHECK_INTERVAL: Duration = Duration::from_millis(100);
const DAEMON_SHUTDOWN_CHECKS: usize = 50;
const DAEMON_SHUTDOWN_CHECK_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Args, Clone)]
pub struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: String,
    #[arg(long, help = "Run the Mini App server as a persistent Q-COLD daemon")]
    daemon: bool,
    #[arg(long, hide = true)]
    daemon_child: bool,
}

pub fn serve(args: &ServeArgs) -> Result<()> {
    if args.daemon && !args.daemon_child {
        return start_daemon(args);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .context("failed to start Mini App async runtime")?
        .block_on(serve_async(args))
}

fn start_daemon(args: &ServeArgs) -> Result<()> {
    let paths = WebappDaemonPaths::new(&args.listen)?;
    fs::create_dir_all(&paths.log_dir)
        .with_context(|| format!("failed to create {}", paths.log_dir.display()))?;
    replace_existing_daemon(&paths, &args.listen)?;

    let executable = env::current_exe().context("failed to locate current Q-COLD executable")?;
    let stdout = daemon_log_file(&paths.stdout_log)?;
    let stderr = daemon_log_file(&paths.stderr_log)?;
    let mut command = Command::new(executable);
    command
        .arg("telegram")
        .arg("serve")
        .arg("--listen")
        .arg(&args.listen)
        .arg("--daemon-child")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    detach_daemon_process(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start Mini App daemon on {}", args.listen))?;
    let pid = child.id();
    fs::write(&paths.pid, format!("{pid}\n"))
        .with_context(|| format!("failed to write {}", paths.pid.display()))?;

    for _ in 0..DAEMON_STARTUP_CHECKS {
        thread::sleep(DAEMON_STARTUP_CHECK_INTERVAL);
        if let Some(status) = child
            .try_wait()
            .context("failed to inspect Mini App daemon startup")?
        {
            fs::remove_file(&paths.pid).ok();
            bail!(
                "Mini App daemon exited during startup with {status}; stderr log: {}",
                paths.stderr_log.display()
            );
        }
    }

    println!(
        "Q-COLD Mini App daemon pid={} listening on http://{}",
        pid, args.listen
    );
    println!("pid_file={}", paths.pid.display());
    println!("stdout_log={}", paths.stdout_log.display());
    println!("stderr_log={}", paths.stderr_log.display());
    Ok(())
}

fn replace_existing_daemon(paths: &WebappDaemonPaths, listen: &str) -> Result<()> {
    let Some(pid) = read_pid_file(&paths.pid)? else {
        return Ok(());
    };
    if !process_alive(pid) {
        fs::remove_file(&paths.pid).ok();
        return Ok(());
    }
    if !process_is_webapp_daemon(pid, listen) {
        bail!(
            "Mini App daemon pid file {} points at pid {pid}, but that process is not the Q-COLD web daemon for {listen}",
            paths.pid.display()
        );
    }
    terminate_process(pid)
        .with_context(|| format!("failed to stop existing Mini App daemon pid {pid}"))?;
    for _ in 0..DAEMON_SHUTDOWN_CHECKS {
        if !process_alive(pid) {
            fs::remove_file(&paths.pid).ok();
            return Ok(());
        }
        thread::sleep(DAEMON_SHUTDOWN_CHECK_INTERVAL);
    }
    bail!("existing Mini App daemon pid {pid} did not stop");
}

fn read_pid_file(path: &Path) -> Result<Option<u32>> {
    if !path.is_file() {
        return Ok(None);
    }
    let value = fs::read_to_string(path)
        .with_context(|| format!("failed to read Mini App daemon pid file {}", path.display()))?;
    let value = value.trim();
    if value.is_empty() {
        fs::remove_file(path).ok();
        return Ok(None);
    }
    Ok(Some(
        value
            .parse()
            .with_context(|| format!("invalid Mini App daemon pid in {}", path.display()))?,
    ))
}

fn process_is_webapp_daemon(pid: u32, listen: &str) -> bool {
    let Ok(cmdline) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return cfg!(not(unix));
    };
    let args = cmdline
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part))
        .collect::<Vec<_>>();
    args.iter().any(|arg| arg == "telegram")
        && args.iter().any(|arg| arg == "serve")
        && args.iter().any(|arg| arg == "--daemon-child")
        && args.windows(2)
            .any(|pair| pair[0] == "--listen" && pair[1] == listen)
}

#[cfg(unix)]
fn detach_daemon_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: the closure only calls async-signal-safe setsid(2) and returns
    // the OS error directly if session detachment fails in the child process.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn detach_daemon_process(_command: &mut Command) {}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: kill(pid, 0) does not send a signal; it only asks the kernel
    // whether a process exists and whether this user may signal it.
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_alive(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> Result<()> {
    let pid = libc::pid_t::try_from(pid).context("Mini App daemon pid exceeds platform range")?;
    // SAFETY: kill(2) is called with a pid read from Q-COLD's own daemon pid
    // file and SIGTERM, which requests normal process termination.
    let result = unsafe { libc::kill(pid, libc::SIGTERM) };
    if result == -1 {
        return Err(io::Error::last_os_error()).context("kill(SIGTERM) failed");
    }
    Ok(())
}

#[cfg(not(unix))]
fn terminate_process(_pid: u32) -> Result<()> {
    bail!("Mini App daemon replacement is only supported on Unix-like systems");
}

fn daemon_log_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open Mini App daemon log {}", path.display()))
}

struct WebappDaemonPaths {
    pid: PathBuf,
    log_dir: PathBuf,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
}

impl WebappDaemonPaths {
    fn new(listen: &str) -> Result<Self> {
        let state = state_dir()?;
        Ok(Self::from_state_dir(&state, listen))
    }

    fn from_state_dir(state: &Path, listen: &str) -> Self {
        let id = sanitize_daemon_id(listen);
        let log_dir = state.join("logs");
        Self {
            pid: state.join(format!("webapp-{id}.pid")),
            stdout_log: log_dir.join(format!("webapp-{id}.out.log")),
            stderr_log: log_dir.join(format!("webapp-{id}.err.log")),
            log_dir,
        }
    }
}

pub fn context_text() -> String {
    let state = dashboard_state();
    [
        "Q-COLD connected repositories".to_string(),
        format!(
            "active\tid={}\tname={}\tpath={}\tbranch={}\tadapter={}",
            state.repository.id,
            state.repository.name,
            state.repository.root,
            state.repository.branch,
            state.repository.adapter,
        ),
        format!("daemon_cwd\t{}", state.daemon_cwd),
        String::new(),
        state
            .repositories
            .iter()
            .map(|repo| {
                format!(
                    "repo\tid={}\tpath={}\tadapter={}\tactive={}",
                    repo.id, repo.root, repo.adapter, repo.active
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        String::new(),
        "Use /app to open the Mini App dashboard.".to_string(),
        format!(
            "Start task template:\n{}",
            state.commands.agent_start_template
        ),
    ]
    .join("\n")
}

async fn serve_async(args: &ServeArgs) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .with_context(|| format!("failed to bind Mini App server on {}", args.listen))?;
    eprintln!("Q-COLD Mini App listening on http://{}", args.listen);
    axum::serve(listener, router())
        .await
        .context("Mini App server failed")
}

fn router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/assets/app.css", get(app_css))
        .route("/assets/app.js", get(app_js))
        .route("/api/state", get(api_state))
        .route("/api/terminal/send", post(api_terminal_send))
        .route("/api/terminal/metadata", post(api_terminal_metadata))
        .route("/api/history", get(api_history))
        .route("/api/events", get(api_events))
        .route("/api/chat", post(api_chat))
        .route("/healthz", get(healthz))
}

async fn index() -> impl IntoResponse {
    no_store(Html(INDEX_HTML))
}

async fn app_css() -> impl IntoResponse {
    no_store(([(CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS))
}

async fn app_js() -> impl IntoResponse {
    no_store((
        [(CONTENT_TYPE, "application/javascript; charset=utf-8")],
        APP_JS,
    ))
}

async fn api_state() -> impl IntoResponse {
    no_store(Json(dashboard_state()))
}

async fn api_history() -> Response {
    match web_history() {
        Ok(entries) => no_store(Json(entries)).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to load history: {err:#}"),
        )
            .into_response(),
    }
}

async fn api_events() -> impl IntoResponse {
    let events = stream::unfold(true, |first| async move {
        if !first {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        let event = match event_snapshot() {
            Ok(snapshot) => serde_json::to_string(&snapshot).map_or_else(
                |err| Event::default().event("error").data(err.to_string()),
                |data| Event::default().event("snapshot").data(data),
            ),
            Err(err) => Event::default().event("error").data(format!("{err:#}")),
        };
        Some((Ok::<Event, Infallible>(event), false))
    });
    no_store(Sse::new(events).keep_alive(KeepAlive::default()))
}

async fn api_chat(headers: HeaderMap, Json(payload): Json<ChatRequest>) -> impl IntoResponse {
    let response = handle_chat_payload(&headers, &payload);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_terminal_send(
    headers: HeaderMap,
    Json(payload): Json<TerminalSendRequest>,
) -> impl IntoResponse {
    let response = handle_terminal_send(&headers, &payload);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_terminal_metadata(
    headers: HeaderMap,
    Json(payload): Json<TerminalMetadataRequest>,
) -> impl IntoResponse {
    let response = handle_terminal_metadata(&headers, &payload);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn healthz() -> &'static str {
    "ok\n"
}

fn no_store<T>(body: T) -> impl IntoResponse
where
    T: IntoResponse,
{
    ([(CACHE_CONTROL, "no-store")], body)
}

fn event_snapshot() -> Result<EventSnapshot> {
    Ok(EventSnapshot {
        state: dashboard_state(),
        history: web_history()?,
    })
}

fn web_history() -> Result<Vec<history::HistoryEntry>> {
    history::load_recent_for_source("web", 20)
}

fn handle_terminal_send(
    headers: &HeaderMap,
    payload: &TerminalSendRequest,
) -> TerminalSendResponse {
    match handle_terminal_send_result(headers, payload) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "sent".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_terminal_send_result(headers: &HeaderMap, payload: &TerminalSendRequest) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let target = payload.target.trim();
    let text = payload.text.trim_end();
    if target.is_empty() {
        bail!("terminal target is empty");
    }
    if text.is_empty() {
        bail!("terminal input is empty");
    }
    if let Some((session, pane)) = parse_zellij_target(target) {
        send_zellij_terminal_input(session, pane, text)?;
        return Ok(());
    }
    send_tmux_terminal_input(target, text)
}

fn handle_terminal_metadata(
    headers: &HeaderMap,
    payload: &TerminalMetadataRequest,
) -> TerminalSendResponse {
    match handle_terminal_metadata_result(headers, payload) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "saved".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_terminal_metadata_result(
    headers: &HeaderMap,
    payload: &TerminalMetadataRequest,
) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let target = clean_terminal_target(&payload.target)?;
    let name = clean_terminal_metadata_value(payload.name.as_deref());
    let scope = clean_terminal_metadata_value(payload.scope.as_deref());
    state::save_terminal_metadata(&target, name.as_deref(), scope.as_deref())
}

fn clean_terminal_target(target: &str) -> Result<String> {
    let target = target.trim();
    if target.is_empty() {
        bail!("terminal target is empty");
    }
    if target.len() > 200 || !target.contains(':') || target.chars().any(char::is_control) {
        bail!("invalid terminal target");
    }
    Ok(target.to_string())
}

fn clean_terminal_metadata_value(value: Option<&str>) -> Option<String> {
    let compact = value?.split_whitespace().collect::<Vec<_>>().join(" ");
    let compact = compact.trim();
    if compact.is_empty() {
        return None;
    }
    Some(truncate_chars(compact, 80))
}

fn send_tmux_terminal_input(target: &str, text: &str) -> Result<()> {
    paste_terminal_text(target, text)?;
    thread::sleep(Duration::from_millis(100));
    let status = Command::new("tmux")
        .args(["send-keys", "-t", target, "Enter"])
        .status()
        .context("failed to submit terminal input through tmux")?;
    if !status.success() {
        bail!("tmux send-keys failed with {status}");
    }
    Ok(())
}

fn send_zellij_terminal_input(session: &str, pane: &str, text: &str) -> Result<()> {
    let status = Command::new("zellij")
        .args(["--session", session, "action", "paste", "--pane-id", pane, text])
        .status()
        .context("failed to paste terminal input through zellij")?;
    if !status.success() {
        bail!("zellij action paste failed with {status}");
    }
    thread::sleep(Duration::from_millis(100));
    let status = Command::new("zellij")
        .args([
            "--session",
            session,
            "action",
            "send-keys",
            "--pane-id",
            pane,
            "Enter",
        ])
        .status()
        .context("failed to submit terminal input through zellij")?;
    if !status.success() {
        bail!("zellij action send-keys failed with {status}");
    }
    Ok(())
}

fn parse_zellij_target(target: &str) -> Option<(&str, &str)> {
    let rest = target.strip_prefix("zellij:")?;
    rest.split_once(':')
}

fn paste_terminal_text(target: &str, text: &str) -> Result<()> {
    let buffer = terminal_paste_buffer_name()?;
    let mut child = Command::new("tmux")
        .args(["load-buffer", "-b", &buffer, "-"])
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to load terminal input into tmux buffer")?;
    let mut stdin = child
        .stdin
        .take()
        .context("failed to open tmux buffer stdin")?;
    stdin
        .write_all(text.as_bytes())
        .context("failed to write terminal input to tmux buffer")?;
    drop(stdin);
    let status = child
        .wait()
        .context("failed waiting for tmux load-buffer")?;
    if !status.success() {
        bail!("tmux load-buffer failed with {status}");
    }

    let status = Command::new("tmux")
        .args(["paste-buffer", "-d", "-b", &buffer, "-t", target])
        .status()
        .context("failed to paste terminal input through tmux")?;
    if !status.success() {
        bail!("tmux paste-buffer failed with {status}");
    }
    Ok(())
}

fn terminal_paste_buffer_name() -> Result<String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_nanos();
    Ok(format!("qcold-web-send-{}-{nanos}", std::process::id()))
}

fn handle_chat_payload(headers: &HeaderMap, payload: &ChatRequest) -> ChatResponse {
    match handle_chat_payload_result(headers, payload) {
        Ok(output) => ChatResponse { ok: true, output },
        Err(err) => ChatResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_chat_payload_result(headers: &HeaderMap, payload: &ChatRequest) -> Result<String> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    route_chat_text(&payload.text)
}

fn require_write_token(headers: &HeaderMap) -> Result<()> {
    let expected = optional_env("QCOLD_WEBAPP_WRITE_TOKEN")
        .context("set QCOLD_WEBAPP_WRITE_TOKEN before enabling GUI command execution")?;
    let provided = headers
        .get("x-qcold-write-token")
        .and_then(|value| value.to_str().ok())
        .context("missing X-QCOLD-Write-Token header")?;
    if provided != expected {
        bail!("invalid GUI write token");
    }
    Ok(())
}

fn webapp_write_token_required() -> bool {
    optional_env("QCOLD_WEBAPP_REQUIRE_WRITE_TOKEN")
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

fn route_chat_text(text: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        bail!("enter a command or prompt");
    }
    history::append("web", "operator", text)?;
    if command_matches(text, "status") {
        return respond(status::telegram_snapshot()?);
    }
    if command_matches(text, "agents") {
        return respond(agents::snapshot()?);
    }
    if command_matches(text, "repos") || command_matches(text, "context") {
        return respond(context_text());
    }
    if command_matches(text, "app") || command_matches(text, "ui") {
        return respond(context_text());
    }
    if command_matches(text, "help") {
        return respond(help_text());
    }
    if command_payload(text, "task").is_some() {
        return respond(
            "/task creates Telegram task topics. Use Telegram for task topics or /agent_start here."
                .to_string(),
        );
    }
    if let Some(request) = command_payload(text, "agent_start") {
        let (track, command) = parse_agent_start(request)?;
        return match agents::start_terminal_shell_agent(track, command) {
            Ok(record) => {
                let output = format!("Started agent:\n{}", agents::snapshot_line(&record));
                history::append("web", "assistant", &output)?;
                Ok(output)
            }
            Err(err) => {
                let output = format!("Failed to start agent: {err:#}");
                history::append("web", "assistant", &output)?;
                Ok(output)
            }
        };
    }
    if text.starts_with('/') {
        bail!("unknown GUI command. Try /status, /repos, /agents, /help, or /agent_start <track> :: <command>");
    }
    let output = run_meta_agent(text)?;
    history::append("web", "assistant", &output)?;
    Ok(output)
}

fn respond(output: String) -> Result<String> {
    history::append("web", "assistant", &output)?;
    Ok(output)
}

fn command_matches(text: &str, command: &str) -> bool {
    text == format!("/{command}") || text.starts_with(&format!("/{command}@"))
}

fn command_payload<'a>(text: &'a str, command: &str) -> Option<&'a str> {
    let (head, rest) = text.split_once(' ').unwrap_or((text, ""));
    if command_matches(head, command) {
        return Some(rest);
    }
    None
}

fn help_text() -> String {
    [
        "Q-COLD Web control plane",
        "/status - show repository task state",
        "/repos - show connected repository context",
        "/agents - show Q-COLD managed agents",
        "/agent_start <track> :: <command> - start an agent through Q-COLD",
        "/app - show dashboard context",
        "/help - show this help",
        "",
        "Plain messages start the configured meta-agent command, or `codex exec --ephemeral --cd <repo> -` by default.",
    ]
    .join("\n")
}

fn parse_agent_start(request: &str) -> Result<(&str, &str)> {
    let Some((track, command)) = request.split_once("::") else {
        bail!("usage: /agent_start <track> :: <command>");
    };
    let track = track.trim();
    let command = command.trim();
    if track.is_empty() || command.is_empty() {
        bail!("usage: /agent_start <track> :: <command>");
    }
    Ok((track, command))
}

fn run_meta_agent(text: &str) -> Result<String> {
    let command = meta_agent_command()?;
    let prompt = history::prompt_context(text, 20)
        .unwrap_or_else(|_| format!("Current operator message:\n{}", text.trim()));
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn meta-agent command: {command}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .context("failed to write prompt to meta-agent command")?;
    }
    let output = child
        .wait_with_output()
        .context("failed to wait for meta-agent command")?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            return Ok("Meta-agent returned no output.".to_string());
        }
        return Ok(stdout);
    }
    Ok(format!(
        "Meta-agent command failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn meta_agent_command() -> Result<String> {
    if let Some(command) = optional_env("QCOLD_META_AGENT_COMMAND") {
        return Ok(command);
    }
    let cwd = repository::active_root()?;
    Ok(format!(
        "codex exec --ephemeral --cd {} -",
        shell_quote(&cwd.display().to_string())
    ))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn sanitize_daemon_id(value: &str) -> String {
    let id = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let id = id.trim_matches('-');
    if id.is_empty() {
        "default".to_string()
    } else {
        id.to_string()
    }
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

fn dashboard_state() -> DashboardState {
    let repository = repository_context();
    let root = repository.root.clone();
    let repositories = repository_contexts();
    DashboardState {
        generated_at_unix: unix_now(),
        daemon_cwd: env::current_dir()
            .map_or_else(|_| "unknown".to_string(), |path| path.display().to_string()),
        repository,
        repositories,
        status: SnapshotBlock::capture("task-flow status", || {
            status::snapshot_for(&PathBuf::from(&root))
        }),
        agents: SnapshotBlock::capture("managed agents", agents::snapshot),
        task_records: task_record_snapshot(),
        host_agents: discover_host_agents(),
        terminals: discover_terminal_sessions(),
        commands: CommandTemplates {
            agent_start_template: format!(
                "/agent_start <track> :: codex exec \"In {root}, start managed task <slug> with cargo qcold task open <slug>, enter the managed task devcontainer, reread AGENTS.md and task logs, then do: <task>. Drive to terminal closeout unless blocked.\""
            ),
        },
    }
}

fn task_record_snapshot() -> TaskRecordSnapshot {
    let sync_error = crate::sync_codex_task_records().err().map(|err| format!("{err:#}"));
    match state::load_task_records(None, 250) {
        Ok(rows) => TaskRecordSnapshot::from_rows(rows, sync_error),
        Err(err) => TaskRecordSnapshot {
            count: 0,
            open: 0,
            closed: 0,
            failed: 0,
            total_displayed_tokens: 0,
            total_output_tokens: 0,
            total_reasoning_tokens: 0,
            total_tool_output_tokens: 0,
            total_large_tool_outputs: 0,
            records: Vec::new(),
            error: Some(format!("{err:#}")),
        },
    }
}

fn discover_terminal_sessions() -> TerminalSnapshot {
    let contexts = terminal_contexts_by_target();
    let metadata = terminal_metadata_by_target();
    let mut records = discover_tmux_terminal_sessions();
    records.extend(discover_zellij_terminal_sessions());
    for pane in &mut records {
        apply_terminal_details(pane, contexts.get(&pane.target), metadata.get(&pane.target));
    }
    TerminalSnapshot {
        count: records.len(),
        records,
    }
}

fn terminal_contexts_by_target() -> HashMap<String, agents::TerminalAgentContext> {
    agents::terminal_contexts()
        .unwrap_or_default()
        .into_iter()
        .map(|context| (context.target.clone(), context))
        .collect()
}

fn terminal_metadata_by_target() -> HashMap<String, state::TerminalMetadataRow> {
    state::load_terminal_metadata()
        .unwrap_or_default()
        .into_iter()
        .map(|metadata| (metadata.target.clone(), metadata))
        .collect()
}

fn apply_terminal_details(
    pane: &mut TerminalPane,
    context: Option<&agents::TerminalAgentContext>,
    metadata: Option<&state::TerminalMetadataRow>,
) {
    let generated = generated_terminal_label(pane, context);
    pane.generated_label.clone_from(&generated);
    pane.name = metadata
        .and_then(|metadata| metadata.name.clone())
        .unwrap_or_default();
    pane.scope = metadata
        .and_then(|metadata| metadata.scope.clone())
        .unwrap_or_default();
    pane.label = if pane.name.is_empty() {
        generated
    } else {
        pane.name.clone()
    };
}

fn generated_terminal_label(
    pane: &TerminalPane,
    context: Option<&agents::TerminalAgentContext>,
) -> String {
    if let Some(context) = context {
        let suffix = short_terminal_id(&context.id);
        if let Some(summary) = terminal_command_summary(&context.command) {
            return format!("{}: {} #{suffix}", context.track, summary);
        }
        return format!("{} #{suffix}", context.track);
    }
    fallback_terminal_label(pane)
}

fn fallback_terminal_label(pane: &TerminalPane) -> String {
    let session = pane
        .session
        .strip_prefix("qcold-")
        .unwrap_or(&pane.session)
        .trim();
    let command = pane.command.trim();
    if command.is_empty() || matches!(command, "fish" | "zellij") {
        return session.to_string();
    }
    format!("{session} - {command}")
}

fn short_terminal_id(id: &str) -> String {
    let last = id.rsplit('-').next().unwrap_or(id);
    let tail = last
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    if tail.is_empty() {
        "term".to_string()
    } else {
        tail
    }
}

fn terminal_command_summary(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    if let Some(quoted) = quoted_command_segments(command).into_iter().next_back() {
        return Some(truncate_chars(&quoted, 56));
    }
    let mut words = command.split_whitespace();
    let first = words.next()?;
    let rest = match first.rsplit('/').next().unwrap_or(first) {
        "c2" | "cc2" => words.collect::<Vec<_>>().join(" "),
        "codex" => {
            let remaining = words.collect::<Vec<_>>();
            if remaining.first().is_some_and(|word| *word == "exec") {
                remaining.get(1..).unwrap_or_default().join(" ")
            } else {
                remaining.join(" ")
            }
        }
        _ => command.to_string(),
    };
    let rest = rest.trim();
    (!rest.is_empty()).then(|| truncate_chars(rest, 56))
}

fn quoted_command_segments(command: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut chars = command.chars();
    while let Some(ch) = chars.next() {
        if ch != '"' && ch != '\'' {
            continue;
        }
        let quote = ch;
        let mut value = String::new();
        let mut escaped = false;
        for inner in chars.by_ref() {
            if escaped {
                value.push(inner);
                escaped = false;
                continue;
            }
            if inner == '\\' {
                escaped = true;
                continue;
            }
            if inner == quote {
                break;
            }
            value.push(inner);
        }
        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        if !value.is_empty() {
            result.push(value);
        }
    }
    result
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn discover_tmux_terminal_sessions() -> Vec<TerminalPane> {
    let Ok(output) = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{session_name}\t#{window_index}.#{pane_index}\t#{pane_pid}\t#{pane_current_command}\t#{pane_current_path}",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_terminal_pane)
        .filter(|pane| {
            pane.session.starts_with("qcold-") || pane.command == "codex" || pane.command == "qcold"
        })
        .map(|mut pane| {
            pane.output = capture_terminal_pane(&pane.target).unwrap_or_default();
            pane
        })
        .collect()
}

fn discover_zellij_terminal_sessions() -> Vec<TerminalPane> {
    let Ok(output) = Command::new("zellij")
        .args(["list-sessions", "--short", "--no-formatting"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|session| session.starts_with("qcold-"))
        .flat_map(discover_zellij_panes)
        .collect()
}

fn parse_terminal_pane(line: &str) -> Option<TerminalPane> {
    let fields = line.splitn(5, '\t').collect::<Vec<_>>();
    if fields.len() != 5 {
        return None;
    }
    let pane = fields[1].to_string();
    Some(TerminalPane::new(
        format!("{}:{pane}", fields[0]),
        fields[0].to_string(),
        pane,
        fields[2].parse().ok()?,
        fields[3].to_string(),
        fields[4].to_string(),
    ))
}

fn discover_zellij_panes(session: &str) -> Vec<TerminalPane> {
    let pid = zellij_session_pid(session).unwrap_or_default();
    let Ok(output) = Command::new("zellij")
        .args(["--session", session, "action", "list-panes"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| parse_zellij_pane(session, pid, line))
        .map(|mut pane| {
            if let Some((session, pane_id)) = parse_zellij_target(&pane.target) {
                pane.output = capture_zellij_pane(session, pane_id).unwrap_or_default();
            }
            pane
        })
        .collect()
}

fn parse_zellij_pane(session: &str, pid: u32, line: &str) -> Option<TerminalPane> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 2 || fields[1] != "terminal" {
        return None;
    }
    let title = fields.get(2).copied().unwrap_or("zellij");
    let expected_title = session.strip_prefix("qcold-").unwrap_or(session);
    if title != expected_title {
        return None;
    }
    let pane = fields[0].to_string();
    Some(TerminalPane::new(
        format!("zellij:{session}:{pane}"),
        session.to_string(),
        pane,
        pid,
        title.to_string(),
        "zellij".to_string(),
    ))
}

fn capture_zellij_pane(session: &str, pane: &str) -> Result<String> {
    let output = Command::new("zellij")
        .args([
            "--session",
            session,
            "action",
            "dump-screen",
            "--ansi",
            "--pane-id",
            pane,
        ])
        .output()
        .with_context(|| format!("failed to dump zellij pane {session}:{pane}"))?;
    if !output.status.success() {
        bail!("zellij dump-screen failed with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
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

fn capture_terminal_pane(target: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args(["capture-pane", "-p", "-e", "-J", "-S", "-160", "-t", target])
        .output()
        .with_context(|| format!("failed to capture tmux pane {target}"))?;
    if !output.status.success() {
        bail!("tmux capture-pane failed with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

fn discover_host_agents() -> HostAgentSnapshot {
    let records = match fs::read_dir("/proc") {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let pid = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
                host_agent_record(pid)
            })
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    HostAgentSnapshot {
        count: records.len(),
        records,
    }
}

fn host_agent_record(pid: u32) -> Option<HostAgentRecord> {
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if cmdline.is_empty() {
        return None;
    }
    let args = cmdline
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect::<Vec<_>>();
    let kind = classify_host_agent(&args)?;
    let cwd = fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map_or_else(|| "unknown".to_string(), |path| path.display().to_string());
    Some(HostAgentRecord {
        pid,
        kind,
        cwd,
        command: compact_command(&args),
    })
}

fn classify_host_agent(args: &[String]) -> Option<String> {
    let executable = args.first().map_or("", String::as_str);
    if command_name(executable) == "codex" {
        return Some("codex".to_string());
    }
    if command_name(executable) == "qcold"
        && args.iter().any(|arg| arg == "telegram")
        && args.iter().any(|arg| arg == "serve")
        && args.iter().any(|arg| arg == "--daemon-child")
    {
        return Some("meta-agent".to_string());
    }
    None
}

fn command_name(command: &str) -> &str {
    command.rsplit('/').next().unwrap_or(command)
}

fn compact_command(args: &[String]) -> String {
    const MAX_COMMAND_LEN: usize = 180;
    let command = args.join(" ");
    if command.len() <= MAX_COMMAND_LEN {
        return command;
    }
    let mut truncated = command
        .chars()
        .take(MAX_COMMAND_LEN.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn repository_context() -> RepositoryContext {
    match repository::active() {
        Ok(repo) => repository_context_from_config(repo),
        Err(_) => fallback_repository_context(),
    }
}

fn repository_contexts() -> Vec<RepositoryContext> {
    repository::list()
        .unwrap_or_default()
        .into_iter()
        .map(repository_context_from_config)
        .collect()
}

fn repository_context_from_config(repo: repository::RepositoryConfig) -> RepositoryContext {
    let root = repo.root.display().to_string();
    let branch = git_output_in(&repo.root, &["branch", "--show-current"])
        .filter(|value| !value.is_empty())
        .or_else(|| git_output_in(&repo.root, &["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let name = root
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("repository")
        .to_string();
    RepositoryContext {
        id: repo.id,
        name,
        root,
        adapter: repo.adapter,
        active: repo.active,
        branch,
        webapp_url: optional_env("QCOLD_TELEGRAM_WEBAPP_URL"),
    }
}

fn fallback_repository_context() -> RepositoryContext {
    let cwd =
        env::current_dir().map_or_else(|_| "unknown".to_string(), |path| path.display().to_string());
    let root = git_output(&["rev-parse", "--show-toplevel"]).unwrap_or_else(|| cwd.clone());
    let branch = git_output(&["branch", "--show-current"])
        .filter(|value| !value.is_empty())
        .or_else(|| git_output(&["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let name = root
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("repository")
        .to_string();
    RepositoryContext {
        id: name.clone(),
        name,
        root,
        adapter: "xtask-process".to_string(),
        active: true,
        branch,
        webapp_url: optional_env("QCOLD_TELEGRAM_WEBAPP_URL"),
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_output_in(root: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[derive(Serialize)]
struct DashboardState {
    generated_at_unix: u64,
    daemon_cwd: String,
    repository: RepositoryContext,
    repositories: Vec<RepositoryContext>,
    status: SnapshotBlock,
    agents: SnapshotBlock,
    task_records: TaskRecordSnapshot,
    host_agents: HostAgentSnapshot,
    terminals: TerminalSnapshot,
    commands: CommandTemplates,
}

#[derive(Serialize)]
struct EventSnapshot {
    state: DashboardState,
    history: Vec<history::HistoryEntry>,
}

#[derive(Serialize)]
struct TaskRecordSnapshot {
    count: usize,
    open: usize,
    closed: usize,
    failed: usize,
    total_displayed_tokens: u64,
    total_output_tokens: u64,
    total_reasoning_tokens: u64,
    total_tool_output_tokens: u64,
    total_large_tool_outputs: u64,
    records: Vec<WebTaskRecord>,
    error: Option<String>,
}

impl TaskRecordSnapshot {
    fn from_rows(rows: Vec<state::TaskRecordRow>, error: Option<String>) -> Self {
        let records = rows.into_iter().map(WebTaskRecord::from_row).collect::<Vec<_>>();
        let count = records.len();
        let open = records
            .iter()
            .filter(|record| record.status == "open")
            .count();
        let failed = records
            .iter()
            .filter(|record| record.status.contains("failed"))
            .count();
        let closed = records
            .iter()
            .filter(|record| record.status.starts_with("closed"))
            .count();
        let total_displayed_tokens = records
            .iter()
            .filter_map(|record| record.token_usage.as_ref())
            .map(|usage| usage.displayed_total_tokens)
            .sum();
        let total_output_tokens = records
            .iter()
            .filter_map(|record| record.token_usage.as_ref())
            .map(|usage| usage.output_tokens)
            .sum();
        let total_reasoning_tokens = records
            .iter()
            .filter_map(|record| record.token_usage.as_ref())
            .map(|usage| usage.reasoning_output_tokens)
            .sum();
        let total_tool_output_tokens = records
            .iter()
            .filter_map(|record| record.token_efficiency.as_ref())
            .map(|efficiency| efficiency.tool_output_original_tokens)
            .sum();
        let total_large_tool_outputs = records
            .iter()
            .filter_map(|record| record.token_efficiency.as_ref())
            .map(|efficiency| efficiency.large_tool_output_calls)
            .sum();
        Self {
            count,
            open,
            closed,
            failed,
            total_displayed_tokens,
            total_output_tokens,
            total_reasoning_tokens,
            total_tool_output_tokens,
            total_large_tool_outputs,
            records,
            error,
        }
    }
}

#[derive(Serialize)]
struct WebTaskRecord {
    id: String,
    source: String,
    sequence: Option<u64>,
    title: String,
    description: String,
    status: String,
    created_at: u64,
    updated_at: u64,
    repo_root: Option<String>,
    cwd: Option<String>,
    agent_id: Option<String>,
    kind: Option<String>,
    codex_thread_id: Option<String>,
    session_path: Option<String>,
    token_usage: Option<TaskTokenUsage>,
    token_efficiency: Option<TaskTokenEfficiency>,
}

impl WebTaskRecord {
    fn from_row(row: state::TaskRecordRow) -> Self {
        let metadata = row
            .metadata_json
            .as_deref()
            .and_then(|value| serde_json::from_str::<Value>(value).ok());
        let token_usage = metadata
            .as_ref()
            .and_then(|value| value.get("token_usage"))
            .map(TaskTokenUsage::from_value);
        let token_efficiency = metadata
            .as_ref()
            .and_then(|value| value.get("token_efficiency"))
            .map(TaskTokenEfficiency::from_value);
        let kind = metadata
            .as_ref()
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let codex_thread_id = metadata
            .as_ref()
            .and_then(|value| value.get("codex_thread_id"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let session_path = metadata
            .as_ref()
            .and_then(|value| value.get("session_path"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        Self {
            id: row.id,
            source: row.source,
            sequence: row.sequence,
            title: row.title,
            description: row.description,
            status: row.status,
            created_at: row.created_at,
            updated_at: row.updated_at,
            repo_root: row.repo_root,
            cwd: row.cwd,
            agent_id: row.agent_id,
            kind,
            codex_thread_id,
            session_path,
            token_usage,
            token_efficiency,
        }
    }
}

#[derive(Clone, Serialize)]
struct TaskTokenUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    non_cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
    displayed_total_tokens: u64,
}

impl TaskTokenUsage {
    fn from_value(value: &Value) -> Self {
        let number = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
        Self {
            input_tokens: number("input_tokens"),
            cached_input_tokens: number("cached_input_tokens"),
            non_cached_input_tokens: number("non_cached_input_tokens"),
            output_tokens: number("output_tokens"),
            reasoning_output_tokens: number("reasoning_output_tokens"),
            total_tokens: number("total_tokens"),
            displayed_total_tokens: number("displayed_total_tokens"),
        }
    }
}

#[derive(Clone, Serialize)]
struct TaskTokenEfficiency {
    session_count: u64,
    tool_output_original_tokens: u64,
    large_tool_output_calls: u64,
    large_tool_output_original_tokens: u64,
}

impl TaskTokenEfficiency {
    fn from_value(value: &Value) -> Self {
        let number = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
        Self {
            session_count: number("session_count"),
            tool_output_original_tokens: number("tool_output_original_tokens"),
            large_tool_output_calls: number("large_tool_output_calls"),
            large_tool_output_original_tokens: number("large_tool_output_original_tokens"),
        }
    }
}

#[derive(Serialize)]
struct RepositoryContext {
    id: String,
    name: String,
    root: String,
    adapter: String,
    active: bool,
    branch: String,
    webapp_url: Option<String>,
}

#[derive(Serialize)]
struct SnapshotBlock {
    label: &'static str,
    ok: bool,
    text: String,
}

impl SnapshotBlock {
    fn capture(label: &'static str, f: impl FnOnce() -> Result<String>) -> Self {
        match f() {
            Ok(text) => Self {
                label,
                ok: true,
                text,
            },
            Err(err) => Self {
                label,
                ok: false,
                text: format!("{err:#}"),
            },
        }
    }
}

#[derive(Serialize)]
struct CommandTemplates {
    agent_start_template: String,
}

#[derive(Serialize)]
struct HostAgentSnapshot {
    count: usize,
    records: Vec<HostAgentRecord>,
}

#[derive(Serialize)]
struct HostAgentRecord {
    pid: u32,
    kind: String,
    cwd: String,
    command: String,
}

#[derive(Default, Serialize)]
struct TerminalSnapshot {
    count: usize,
    records: Vec<TerminalPane>,
}

#[derive(Serialize)]
struct TerminalPane {
    target: String,
    session: String,
    pane: String,
    pid: u32,
    command: String,
    cwd: String,
    label: String,
    generated_label: String,
    name: String,
    scope: String,
    output: String,
}

impl TerminalPane {
    fn new(
        target: String,
        session: String,
        pane: String,
        pid: u32,
        command: String,
        cwd: String,
    ) -> Self {
        let mut pane = Self {
            target,
            session,
            pane,
            pid,
            command,
            cwd,
            label: String::new(),
            generated_label: String::new(),
            name: String::new(),
            scope: String::new(),
            output: String::new(),
        };
        apply_terminal_details(&mut pane, None, None);
        pane
    }
}

#[derive(Deserialize)]
struct ChatRequest {
    text: String,
}

#[derive(Deserialize)]
struct TerminalSendRequest {
    target: String,
    text: String,
}

#[derive(Deserialize)]
struct TerminalMetadataRequest {
    target: String,
    name: Option<String>,
    scope: Option<String>,
}

#[derive(Serialize)]
struct ChatResponse {
    ok: bool,
    output: String,
}

#[derive(Serialize)]
struct TerminalSendResponse {
    ok: bool,
    output: String,
}

const INDEX_HTML: &str = include_str!("webapp_assets/index.html");
const APP_CSS: &str = include_str!("webapp_assets/app.css");
const APP_JS: &str = include_str!("webapp_assets/app.js");

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn daemon_paths_are_scoped_by_listen_address() {
        let temp = tempdir().unwrap();
        let paths = WebappDaemonPaths::from_state_dir(temp.path(), "192.0.2.10:8787");
        assert_eq!(
            paths.pid,
            temp.path().join("webapp-192-0-2-10-8787.pid")
        );
        assert_eq!(
            paths.stdout_log,
            temp.path()
                .join("logs")
                .join("webapp-192-0-2-10-8787.out.log")
        );
        assert_eq!(
            paths.stderr_log,
            temp.path()
                .join("logs")
                .join("webapp-192-0-2-10-8787.err.log")
        );
    }

    #[test]
    fn empty_daemon_path_id_falls_back_to_default() {
        assert_eq!(sanitize_daemon_id(":::////"), "default");
    }

    #[test]
    fn host_agent_classifier_detects_console_codex() {
        let args = vec![
            "/opt/qcold-demo/bin/codex".to_string(),
            "exec".to_string(),
            "inspect".to_string(),
        ];
        assert_eq!(classify_host_agent(&args).as_deref(), Some("codex"));
    }

    #[test]
    fn host_agent_classifier_ignores_codex_node_wrapper() {
        let args = vec![
            "node".to_string(),
            "/opt/qcold-demo/bin/codex".to_string(),
            "exec".to_string(),
        ];
        assert_eq!(classify_host_agent(&args), None);
    }

    #[test]
    fn host_agent_classifier_ignores_xtask_taskflow_processes() {
        let args = vec![
            "/tmp/repository-taskflow/example/debug/xtask".to_string(),
            "task".to_string(),
            "enter".to_string(),
        ];
        assert_eq!(classify_host_agent(&args), None);
    }

    #[test]
    fn host_agent_classifier_detects_qcold_meta_daemon() {
        let args = vec![
            "/opt/qcold-demo/bin/qcold".to_string(),
            "telegram".to_string(),
            "serve".to_string(),
            "--listen".to_string(),
            "127.0.0.1:8787".to_string(),
            "--daemon-child".to_string(),
        ];
        assert_eq!(classify_host_agent(&args).as_deref(), Some("meta-agent"));
    }

    #[test]
    fn terminal_pane_parser_builds_tmux_target() {
        let pane = parse_terminal_pane("main\t0.1\t123\tcodex\t/workspace/repo").unwrap();
        assert_eq!(pane.target, "main:0.1");
        assert_eq!(pane.pid, 123);
        assert_eq!(pane.command, "codex");
        assert_eq!(pane.label, "main - codex");
    }

    #[test]
    fn terminal_command_summary_uses_wrapped_agent_prompt() {
        assert_eq!(
            terminal_command_summary("cc2 \"refactor terminal naming\"").as_deref(),
            Some("refactor terminal naming")
        );
        assert_eq!(
            terminal_command_summary("codex exec \"inspect terminal panes\"").as_deref(),
            Some("inspect terminal panes")
        );
    }

    #[test]
    fn terminal_metadata_override_becomes_display_label() {
        let mut pane = TerminalPane::new(
            "zellij:qcold-c2-1234:terminal_0".to_string(),
            "qcold-c2-1234".to_string(),
            "terminal_0".to_string(),
            42,
            "c2-1234".to_string(),
            "/repo".to_string(),
        );
        let metadata = state::TerminalMetadataRow {
            target: pane.target.clone(),
            name: Some("client migration".to_string()),
            scope: Some("review".to_string()),
            updated_at: 123,
        };

        apply_terminal_details(&mut pane, None, Some(&metadata));

        assert_eq!(pane.generated_label, "c2-1234 - c2-1234");
        assert_eq!(pane.label, "client migration");
        assert_eq!(pane.name, "client migration");
        assert_eq!(pane.scope, "review");
    }

    #[test]
    fn terminal_metadata_values_are_compacted_and_limited() {
        assert_eq!(
            clean_terminal_metadata_value(Some("  refactoring\n  terminal labels  ")).as_deref(),
            Some("refactoring terminal labels")
        );
        assert_eq!(clean_terminal_metadata_value(Some(" \n\t ")), None);
    }
}
