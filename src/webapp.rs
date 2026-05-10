use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use axum::{
    extract::{Json, Query},
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
const AGENT_LIMIT_CACHE_TTL: u64 = 600;
const AGENT_LIMIT_STATUS_ATTEMPTS: usize = 2;
const AGENT_LIMIT_STATUS_TIMEOUT: u64 = 20;
const WEB_QUEUE_RETRY_DELAYS: [u64; 3] = [60, 300, 600];
static AGENT_LIMIT_CACHE: OnceLock<Mutex<Option<AgentLimitCache>>> = OnceLock::new();
static WEB_QUEUE_WORKERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

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
        .route("/api/agent-limits", get(api_agent_limits))
        .route("/api/task-transcript", get(api_task_transcript))
        .route("/api/task-chat/target", post(api_task_chat_target))
        .route("/api/task-chat/send", post(api_task_chat_send))
        .route("/api/queue/run", post(api_queue_run))
        .route("/api/queue/append", post(api_queue_append))
        .route("/api/queue/remove", post(api_queue_remove))
        .route("/api/queue/clear", post(api_queue_clear))
        .route("/api/queue/stop", post(api_queue_stop))
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

async fn api_agent_limits(Query(query): Query<AgentLimitQuery>) -> impl IntoResponse {
    let refresh = query.refresh.as_deref() == Some("true");
    no_store(Json(agent_limit_snapshot(refresh)))
}

async fn api_task_transcript(Query(query): Query<TaskTranscriptQuery>) -> impl IntoResponse {
    let response = task_transcript_response(&query.id);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_queue_run(headers: HeaderMap, Json(payload): Json<QueueRunRequest>) -> impl IntoResponse {
    let response = handle_queue_run(&headers, payload);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_queue_append(
    headers: HeaderMap,
    Json(payload): Json<QueueAppendRequest>,
) -> impl IntoResponse {
    let response = handle_queue_append(&headers, payload);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_queue_remove(
    headers: HeaderMap,
    Json(payload): Json<QueueRemoveRequest>,
) -> impl IntoResponse {
    let response = handle_queue_remove(&headers, &payload);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_queue_clear(
    headers: HeaderMap,
    Json(payload): Json<QueueClearRequest>,
) -> impl IntoResponse {
    let response = handle_queue_clear(&headers, &payload);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_queue_stop(headers: HeaderMap) -> impl IntoResponse {
    let response = handle_queue_stop(&headers);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_task_chat_target(
    headers: HeaderMap,
    Json(payload): Json<TaskChatTargetRequest>,
) -> impl IntoResponse {
    let response = handle_task_chat_target(&headers, &payload);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_task_chat_send(
    headers: HeaderMap,
    Json(payload): Json<TaskChatSendRequest>,
) -> impl IntoResponse {
    let response = handle_task_chat_send(&headers, &payload);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
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
    history::load_recent_meta_visible_for_source("web", 20)
}

fn handle_queue_run(headers: &HeaderMap, payload: QueueRunRequest) -> TerminalSendResponse {
    match handle_queue_run_result(headers, payload) {
        Ok(run_id) => TerminalSendResponse {
            ok: true,
            output: format!("queue-run\t{run_id}"),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_run_result(headers: &HeaderMap, payload: QueueRunRequest) -> Result<String> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let selected_agent_command = payload.selected_agent_command.trim();
    if selected_agent_command.is_empty() {
        bail!("queue agent command is empty");
    }
    if !agents::available_agent_commands()
        .iter()
        .any(|agent| agent.command == selected_agent_command)
    {
        bail!("unknown queue agent command: {selected_agent_command}");
    }
    let prompts = payload
        .items
        .into_iter()
        .filter(|item| !item.prompt.trim().is_empty())
        .collect::<Vec<_>>();
    if prompts.is_empty() {
        bail!("queue has no runnable items");
    }
    let fallback_run_id = base36_time_id();
    let run_id = clean_queue_run_id(
        payload
            .run_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&fallback_run_id),
    );
    let now = unix_now();
    let track = queue_track(&run_id);
    let run = state::QueueRunRow {
        id: run_id.clone(),
        status: "running".to_string(),
        selected_agent_command: selected_agent_command.to_string(),
        selected_repo_root: payload.selected_repo_root.filter(|value| !value.trim().is_empty()),
        selected_repo_name: payload.selected_repo_name.filter(|value| !value.trim().is_empty()),
        track,
        current_index: -1,
        stop_requested: false,
        message: "queued".to_string(),
        created_at: now,
        updated_at: now,
    };
    let mut used_slugs = HashSet::new();
    let items = prompts
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let fallback_slug = queue_slug(&run_id, index);
            let slug = clean_queue_slug(
                item.slug
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(&fallback_slug),
                &run_id,
                index,
                &mut used_slugs,
            );
            state::QueueItemRow {
                id: item
                    .id
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| format!("queue-{run_id}-{}", index + 1)),
                run_id: run_id.clone(),
                position: i64::try_from(index).unwrap_or(i64::MAX),
                prompt: item.prompt.trim().to_string(),
                slug,
                repo_root: item
                    .repo_root
                    .or_else(|| run.selected_repo_root.clone())
                    .filter(|value| !value.trim().is_empty()),
                repo_name: item
                    .repo_name
                    .or_else(|| run.selected_repo_name.clone())
                    .filter(|value| !value.trim().is_empty()),
                agent_command: item
                    .agent_command
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| selected_agent_command.to_string()),
                agent_id: None,
                status: "pending".to_string(),
                message: String::new(),
                attempts: 0,
                next_attempt_at: None,
                started_at: now,
                updated_at: now,
            }
        })
        .collect::<Vec<_>>();
    state::replace_web_queue(&run, &items)?;
    spawn_web_queue_worker(run_id.clone());
    Ok(run_id)
}

fn handle_queue_append(headers: &HeaderMap, payload: QueueAppendRequest) -> TerminalSendResponse {
    match handle_queue_append_result(headers, payload) {
        Ok(count) => TerminalSendResponse {
            ok: true,
            output: format!("appended {count} queue item(s)"),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_append_result(headers: &HeaderMap, payload: QueueAppendRequest) -> Result<usize> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let run_id = clean_queue_run_id(&payload.run_id);
    let (run, existing_items) = state::load_web_queue_run(&run_id)?;
    let Some(run) = run else {
        bail!("unknown queue run: {run_id}");
    };
    if !matches!(run.status.as_str(), "running" | "waiting" | "starting") {
        bail!("queue is not running");
    }
    let prompts = payload
        .items
        .into_iter()
        .filter(|item| !item.prompt.trim().is_empty())
        .collect::<Vec<_>>();
    if prompts.is_empty() {
        bail!("queue append has no runnable items");
    }
    let now = unix_now();
    let mut used_slugs = existing_items
        .iter()
        .map(|item| item.slug.clone())
        .collect::<HashSet<_>>();
    let start_position = existing_items
        .iter()
        .map(|item| item.position)
        .max()
        .unwrap_or(-1)
        .saturating_add(1);
    let items = prompts
        .into_iter()
        .enumerate()
        .map(|(offset, item)| {
            let index = usize::try_from(start_position)
                .unwrap_or(0)
                .saturating_add(offset);
            let fallback_slug = queue_slug(&run_id, index);
            let slug = clean_queue_slug(
                item.slug
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(&fallback_slug),
                &run_id,
                index,
                &mut used_slugs,
            );
            state::QueueItemRow {
                id: item
                    .id
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| format!("queue-{run_id}-{}", index + 1)),
                run_id: run_id.clone(),
                position: start_position.saturating_add(i64::try_from(offset).unwrap_or(0)),
                prompt: item.prompt.trim().to_string(),
                slug,
                repo_root: item
                    .repo_root
                    .or_else(|| run.selected_repo_root.clone())
                    .filter(|value| !value.trim().is_empty()),
                repo_name: item
                    .repo_name
                    .or_else(|| run.selected_repo_name.clone())
                    .filter(|value| !value.trim().is_empty()),
                agent_command: item
                    .agent_command
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| run.selected_agent_command.clone()),
                agent_id: None,
                status: "pending".to_string(),
                message: String::new(),
                attempts: 0,
                next_attempt_at: None,
                started_at: now,
                updated_at: now,
            }
        })
        .collect::<Vec<_>>();
    let count = items.len();
    state::append_web_queue_items(&run_id, &items)?;
    spawn_web_queue_worker(run_id);
    Ok(count)
}

fn handle_queue_stop(headers: &HeaderMap) -> TerminalSendResponse {
    match handle_queue_stop_result(headers) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "queue stop requested".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_stop_result(headers: &HeaderMap) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    state::request_web_queue_stop()
}

fn handle_queue_remove(headers: &HeaderMap, payload: &QueueRemoveRequest) -> TerminalSendResponse {
    match handle_queue_remove_result(headers, payload) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "removed".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_remove_result(headers: &HeaderMap, payload: &QueueRemoveRequest) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let run_id = clean_queue_run_id(&payload.run_id);
    let item_id = payload.item_id.trim();
    if item_id.is_empty() || item_id.chars().any(char::is_control) {
        bail!("invalid queue item id");
    }
    let (run, _) = state::load_web_queue_run(&run_id)?;
    if run.as_ref().is_some_and(|run| {
        matches!(run.status.as_str(), "running" | "waiting" | "starting" | "stopping")
    }) {
        bail!("cannot remove queue items while the queue is running");
    }
    let task_id = payload
        .task_id
        .as_deref()
        .filter(|id| !id.trim().is_empty());
    let agent_id = payload
        .agent_id
        .as_deref()
        .filter(|id| !id.trim().is_empty());
    match state::delete_web_queue_item_if_exists(&run_id, item_id)? {
        Some(item) => cleanup_queue_item_artifacts(&item, task_id, agent_id),
        None => cleanup_task_agent_artifacts(task_id, agent_id),
    }
}

fn handle_queue_clear(headers: &HeaderMap, payload: &QueueClearRequest) -> TerminalSendResponse {
    match handle_queue_clear_result(headers, payload) {
        Ok(count) => TerminalSendResponse {
            ok: true,
            output: format!("cleared {count} queue item(s)"),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_clear_result(headers: &HeaderMap, payload: &QueueClearRequest) -> Result<usize> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let requested_run_id = payload
        .run_id
        .as_deref()
        .map(clean_queue_run_id)
        .filter(|run_id| !run_id.is_empty());
    let (run, items) = match requested_run_id {
        Some(run_id) => state::load_web_queue_run(&run_id)?,
        None => state::load_web_queue()?,
    };
    let Some(run) = run else {
        return Ok(0);
    };
    if matches!(
        run.status.as_str(),
        "running" | "waiting" | "starting" | "stopping"
    ) {
        state::request_web_queue_stop()?;
    }
    let mut removed = 0;
    for item in items {
        let item = state::delete_web_queue_item(&run.id, &item.id)?;
        cleanup_queue_item_artifacts(&item, None, None)?;
        removed += 1;
    }
    Ok(removed)
}

fn cleanup_queue_item_artifacts(
    item: &state::QueueItemRow,
    task_id: Option<&str>,
    agent_id: Option<&str>,
) -> Result<()> {
    let default_task_id = format!("task/{}", item.slug);
    let task_id = task_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .unwrap_or(default_task_id);
    let task = state::get_task_record(&task_id)?;
    let agent_id = agent_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .or_else(|| item.agent_id.clone())
        .or_else(|| task.as_ref().and_then(|task| task.agent_id.clone()));
    cleanup_existing_task_agent_artifacts(&task_id, task, agent_id)
}

fn cleanup_task_agent_artifacts(task_id: Option<&str>, agent_id: Option<&str>) -> Result<()> {
    let task_id = task_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string);
    let task = task_id
        .as_deref()
        .map(state::get_task_record)
        .transpose()?
        .flatten();
    let agent_id = agent_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .or_else(|| task.as_ref().and_then(|task| task.agent_id.clone()));
    if let Some(task_id) = task_id {
        cleanup_existing_task_agent_artifacts(&task_id, task, agent_id)?;
    } else if let Some(agent_id) = agent_id {
        let _ = agents::terminate_agent(&agent_id);
    }
    Ok(())
}

fn cleanup_existing_task_agent_artifacts(
    task_id: &str,
    task: Option<state::TaskRecordRow>,
    agent_id: Option<String>,
) -> Result<()> {
    if task.is_some() {
        state::delete_task_record(task_id)?;
    }
    if let Some(agent_id) = agent_id {
        let _ = agents::terminate_agent(&agent_id);
    }
    Ok(())
}

fn spawn_web_queue_worker(run_id: String) {
    let workers = WEB_QUEUE_WORKERS.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut active) = workers.lock() {
        if !active.insert(run_id.clone()) {
            return;
        }
    }
    thread::spawn(move || {
        if let Err(err) = run_web_queue(&run_id) {
            let _ = state::update_web_queue_run(&run_id, "failed", -1, &format!("{err:#}"));
        }
        if let Some(workers) = WEB_QUEUE_WORKERS.get() {
            if let Ok(mut active) = workers.lock() {
                active.remove(&run_id);
            }
        }
    });
}

fn web_queue_worker_active(run_id: &str) -> bool {
    WEB_QUEUE_WORKERS
        .get()
        .and_then(|workers| workers.lock().ok())
        .is_some_and(|active| active.contains(run_id))
}

fn run_web_queue(run_id: &str) -> Result<()> {
    state::update_web_queue_run(run_id, "running", -1, "running")?;
    loop {
        let (run, items) = state::load_web_queue_run(run_id)?;
        if run.is_none() {
            return Ok(());
        }
        if items.is_empty() {
            state::update_web_queue_run(run_id, "failed", -1, "queue has no items")?;
            return Ok(());
        }
        let Some(item) = items.into_iter().find(|item| !queue_item_terminal(&item.status)) else {
            state::update_web_queue_run(run_id, "success", -1, "closed successfully")?;
            return Ok(());
        };
        let index = item.position;
        if state::web_queue_stop_requested(run_id)? {
            state::update_web_queue_item(
                run_id,
                &item.id,
                "stopped",
                "stopped by operator",
                None,
                item.attempts,
                None,
            )?;
            state::update_web_queue_run(run_id, "stopped", index, "stopped by operator")?;
            return Ok(());
        }
        state::update_web_queue_run(run_id, "running", index, &format!("running {}", item.slug))?;
        match run_web_queue_item(run_id, &item)? {
            QueueItemOutcome::Success => {}
            QueueItemOutcome::Stopped => {
                state::update_web_queue_run(run_id, "stopped", index, "stopped by operator")?;
                return Ok(());
            }
            QueueItemOutcome::Failed { message, .. } => {
                state::update_web_queue_run(run_id, "failed", index, &message)?;
                return Ok(());
            }
        }
    }
}

fn queue_item_terminal(status: &str) -> bool {
    matches!(status, "success" | "failed" | "stopped" | "blocked")
}

enum QueueItemOutcome {
    Success,
    Stopped,
    Failed { message: String, retryable: bool },
}

impl QueueItemOutcome {
    fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
            retryable: false,
        }
    }

    fn retryable_failure(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
            retryable: true,
        }
    }
}

fn run_web_queue_item(run_id: &str, item: &state::QueueItemRow) -> Result<QueueItemOutcome> {
    if let Some(status) = queue_task_status(item)? {
        if status == "closed:success" {
            update_successful_queue_item(run_id, item, item.agent_id.as_deref(), item.attempts)?;
            return Ok(QueueItemOutcome::Success);
        }
        if status.starts_with("closed") {
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                &status,
                None,
                item.attempts,
                None,
            )?;
            return Ok(QueueItemOutcome::failed(status));
        }
    }
    if matches!(item.status.as_str(), "running" | "starting") {
        if let Some(agent_id) = item.agent_id.as_deref() {
            if agent_running(agent_id) {
                return wait_for_queue_item_closeout(run_id, item, agent_id, item.attempts);
            }
            let message = "agent exited before task closeout";
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                message,
                Some(agent_id),
                item.attempts,
                None,
            )?;
            return Ok(QueueItemOutcome::failed(message));
        }
    }

    let mut retries = item.attempts.max(0);
    loop {
        if state::web_queue_stop_requested(run_id)? {
            state::update_web_queue_item(
                run_id,
                &item.id,
                "stopped",
                "stopped by operator",
                None,
                retries,
                None,
            )?;
            return Ok(QueueItemOutcome::Stopped);
        }

        if let Some(limit) = queue_agent_limit_for_command(&item.agent_command) {
            if limit.state != "ok" {
                if limit.state == "unauthenticated" || retries as usize >= WEB_QUEUE_RETRY_DELAYS.len() {
                    let message = format!(
                        "{} is {}: {}",
                        item.agent_command, limit.state, limit.summary
                    );
                    state::update_web_queue_item(
                        run_id,
                        &item.id,
                        "failed",
                        &message,
                        None,
                        retries,
                        None,
                    )?;
                    return Ok(QueueItemOutcome::failed(message));
                }
                let delay = WEB_QUEUE_RETRY_DELAYS[retries as usize];
                retries += 1;
                let next_attempt_at = unix_now().saturating_add(delay);
                let message = format!(
                    "{} is {}: {}; retry {}/{} in {}s",
                    item.agent_command,
                    limit.state,
                    limit.summary,
                    retries,
                    WEB_QUEUE_RETRY_DELAYS.len(),
                    delay
                );
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "waiting",
                    &message,
                    None,
                    retries,
                    Some(next_attempt_at),
                )?;
                if !sleep_queue_retry(run_id, delay)? {
                    state::update_web_queue_item(
                        run_id,
                        &item.id,
                        "stopped",
                        "stopped by operator",
                        None,
                        retries,
                        None,
                    )?;
                    return Ok(QueueItemOutcome::Stopped);
                }
                continue;
            }
        } else {
            let message = format!("unknown queue agent command: {}", item.agent_command);
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                &message,
                None,
                retries,
                None,
            )?;
            return Ok(QueueItemOutcome::failed(message));
        }

        state::update_web_queue_item(
            run_id,
            &item.id,
            "starting",
            "starting clean agent context",
            None,
            retries,
            None,
        )?;
        match start_web_queue_item(run_id, item, retries)? {
            QueueItemOutcome::Failed {
                message,
                retryable: true,
            } if (retries as usize) < WEB_QUEUE_RETRY_DELAYS.len() =>
            {
                let delay = WEB_QUEUE_RETRY_DELAYS[retries as usize];
                retries += 1;
                let next_attempt_at = unix_now().saturating_add(delay);
                let retry_message = format!(
                    "{message}; retry {}/{} in {}s",
                    retries,
                    WEB_QUEUE_RETRY_DELAYS.len(),
                    delay
                );
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "waiting",
                    &retry_message,
                    None,
                    retries,
                    Some(next_attempt_at),
                )?;
                if !sleep_queue_retry(run_id, delay)? {
                    return Ok(QueueItemOutcome::Stopped);
                }
            }
            outcome => return Ok(outcome),
        }
    }
}

fn start_web_queue_item(
    run_id: &str,
    item: &state::QueueItemRow,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    let request = AgentStartRequest {
        cwd: item.repo_root.as_ref().map(PathBuf::from),
        track: queue_track(run_id),
        command: item.agent_command.clone(),
    };
    let agent = match start_web_agent(&request) {
        Ok(agent) => agent,
        Err(err) => return Ok(QueueItemOutcome::retryable_failure(format!("{err:#}"))),
    };
    state::update_web_queue_item(
        run_id,
        &item.id,
        "starting",
        "waiting for agent terminal",
        Some(&agent.id),
        attempts,
        None,
    )?;
    let Some(target) = wait_for_agent_terminal_target(&agent.id) else {
        return Ok(QueueItemOutcome::retryable_failure(
            "agent terminal did not appear",
        ));
    };
    set_queue_terminal_scope(&target, item)?;
    thread::sleep(Duration::from_secs(1));
    if let Err(err) = send_terminal_text_to_target(&target, "/new") {
        return Ok(QueueItemOutcome::retryable_failure(format!("{err:#}")));
    }
    thread::sleep(Duration::from_millis(500));
    if let Err(err) = send_terminal_text_to_target(&target, &queue_task_instruction(item)) {
        return Ok(QueueItemOutcome::retryable_failure(format!("{err:#}")));
    }
    state::update_web_queue_item(
        run_id,
        &item.id,
        "running",
        &format!("agent {}", agent.id),
        Some(&agent.id),
        attempts,
        None,
    )?;
    wait_for_queue_item_closeout(run_id, item, &agent.id, attempts)
}

fn set_queue_terminal_scope(target: &str, item: &state::QueueItemRow) -> Result<()> {
    let metadata = terminal_metadata_by_target();
    let name = metadata.get(target).and_then(|metadata| metadata.name.as_deref());
    let scope = queue_terminal_scope(item);
    state::save_terminal_metadata(target, name, Some(&scope))
}

fn queue_terminal_scope(item: &state::QueueItemRow) -> String {
    format!("task/{}", item.slug)
}

fn wait_for_queue_item_closeout(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    loop {
        if state::web_queue_stop_requested(run_id)? {
            state::update_web_queue_item(
                run_id,
                &item.id,
                "stopped",
                "stopped by operator",
                Some(agent_id),
                attempts,
                None,
            )?;
            return Ok(QueueItemOutcome::Stopped);
        }
        thread::sleep(Duration::from_secs(5));
        crate::sync_codex_task_records().ok();
        if let Some(status) = queue_task_status(item)? {
            if status == "closed:success" {
                update_successful_queue_item(run_id, item, Some(agent_id), attempts)?;
                return Ok(QueueItemOutcome::Success);
            }
            if status.starts_with("closed") {
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "failed",
                    &status,
                    Some(agent_id),
                    attempts,
                    None,
                )?;
                return Ok(QueueItemOutcome::failed(status));
            }
            if status == "open" && !agent_running(agent_id) {
                let message = "agent exited before task closeout".to_string();
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "failed",
                    &message,
                    Some(agent_id),
                    attempts,
                    None,
                )?;
                return Ok(QueueItemOutcome::failed(message));
            }
        } else if !agent_running(agent_id) {
            let message = "agent exited before opening task record".to_string();
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                &message,
                Some(agent_id),
                attempts,
                None,
            )?;
            return Ok(QueueItemOutcome::retryable_failure(message));
        }
    }
}

fn update_successful_queue_item(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: Option<&str>,
    attempts: i64,
) -> Result<()> {
    let message = agent_id.map_or_else(
        || "closed successfully".to_string(),
        |agent_id| format!("closed successfully; {}", cleanup_queue_agent(agent_id)),
    );
    state::update_web_queue_item(
        run_id,
        &item.id,
        "success",
        &message,
        agent_id,
        attempts,
        None,
    )
}

fn cleanup_queue_agent(agent_id: &str) -> String {
    match agents::terminate_agent(agent_id) {
        Ok(true) => "agent terminal closed".to_string(),
        Ok(false) => "agent already stopped".to_string(),
        Err(err) => format!("agent cleanup failed: {err:#}"),
    }
}

fn reconcile_stale_web_queue_run() -> Result<()> {
    let (run, items) = state::load_web_queue()?;
    let Some(run) = run else {
        return Ok(());
    };
    if !matches!(
        run.status.as_str(),
        "running" | "waiting" | "starting" | "stopping"
    ) || web_queue_worker_active(&run.id)
    {
        return Ok(());
    }

    crate::sync_codex_task_records().ok();
    cleanup_orphaned_queue_agents(&run, &items);
    for item in items {
        if let Some(status) = queue_task_status(&item)? {
            if status == "closed:success" {
                if item.status != "success"
                    || item
                        .agent_id
                        .as_deref()
                        .is_some_and(agent_running)
                {
                    update_successful_queue_item(
                        &run.id,
                        &item,
                        item.agent_id.as_deref(),
                        item.attempts,
                    )?;
                }
                continue;
            }
            if status.starts_with("closed") && item.status != "success" {
                state::update_web_queue_item(
                    &run.id,
                    &item.id,
                    "failed",
                    &status,
                    item.agent_id.as_deref(),
                    item.attempts,
                    None,
                )?;
                state::update_web_queue_run(&run.id, "failed", item.position, &status)?;
                return Ok(());
            }
        }

        if item.status == "success" {
            if item
                .agent_id
                .as_deref()
                .is_some_and(agent_running)
            {
                update_successful_queue_item(
                    &run.id,
                    &item,
                    item.agent_id.as_deref(),
                    item.attempts,
                )?;
            }
            continue;
        }
        if let Some(agent_id) = item.agent_id.as_deref() {
            if matches!(item.status.as_str(), "running" | "starting") && !agent_running(agent_id) {
                let message = "agent exited before task closeout";
                state::update_web_queue_item(
                    &run.id,
                    &item.id,
                    "failed",
                    message,
                    Some(agent_id),
                    item.attempts,
                    None,
                )?;
                state::update_web_queue_run(&run.id, "failed", item.position, message)?;
                return Ok(());
            }
        }
        state::update_web_queue_run(
            &run.id,
            "running",
            item.position,
            &format!("running {}", item.slug),
        )?;
        spawn_web_queue_worker(run.id.clone());
        return Ok(());
    }

    state::update_web_queue_run(&run.id, "success", -1, "closed successfully")?;
    Ok(())
}

fn cleanup_orphaned_queue_agents(run: &state::QueueRunRow, items: &[state::QueueItemRow]) {
    let known_agents = items
        .iter()
        .filter_map(|item| item.agent_id.as_deref())
        .collect::<HashSet<_>>();
    let track = queue_track(&run.id);
    let Ok(contexts) = agents::terminal_contexts() else {
        return;
    };
    for context in contexts {
        if context.track == track && !known_agents.contains(context.id.as_str()) {
            let _ = agents::terminate_agent(&context.id);
        }
    }
}

fn queue_agent_limit_for_command(command: &str) -> Option<AgentLimitRecord> {
    let agent = agents::available_agent_commands()
        .into_iter()
        .find(|agent| agent.command == command)?;
    Some(probe_agent_limit(&agent))
}

fn queue_task_status(item: &state::QueueItemRow) -> Result<Option<String>> {
    let task_id = format!("task/{}", item.slug);
    let Some(record) = state::get_task_record(&task_id)? else {
        return Ok(None);
    };
    if item
        .repo_root
        .as_deref()
        .is_some_and(|repo| record.repo_root.as_deref().is_some_and(|value| value != repo))
    {
        return Ok(None);
    }
    Ok(Some(record.status))
}

fn agent_running(agent_id: &str) -> bool {
    agents::running_snapshot()
        .map(|snapshot| snapshot.contains(&format!("agent\t{agent_id}\t")))
        .unwrap_or(false)
}

fn wait_for_agent_terminal_target(agent_id: &str) -> Option<String> {
    for _ in 0..20 {
        if let Some(target) = agents::terminal_contexts()
            .ok()?
            .into_iter()
            .find(|context| context.id == agent_id)
            .map(|context| context.target)
        {
            return Some(target);
        }
        thread::sleep(Duration::from_millis(500));
    }
    None
}

fn send_terminal_text_to_target(target: &str, text: &str) -> Result<()> {
    send_terminal_paste(target, text, true)
}

fn sleep_queue_retry(run_id: &str, delay_seconds: u64) -> Result<bool> {
    let mut slept = 0;
    while slept < delay_seconds {
        if state::web_queue_stop_requested(run_id)? {
            return Ok(false);
        }
        let step = (delay_seconds - slept).min(5);
        thread::sleep(Duration::from_secs(step));
        slept += step;
    }
    Ok(true)
}

fn queue_task_instruction(item: &state::QueueItemRow) -> String {
    let root = item.repo_root.as_deref().unwrap_or("<repo>");
    format!(
        "Use the launched host-side agent workspace as your home base for {root}; do not enter a devcontainer from $QCOLD_AGENT_WORKTREE. Start managed task {slug} with cargo qcold task open {slug}, enter that managed task worktree and its devcontainer if the task flow provides one, reread AGENTS.md and task logs, then do: {prompt} Drive the task to terminal closeout unless blocked. After closeout, cd back to $QCOLD_AGENT_WORKTREE before starting a new chat or task.",
        slug = item.slug,
        prompt = item.prompt.trim(),
    )
}

fn clean_queue_run_id(value: &str) -> String {
    sanitize_daemon_id(value)
}

fn clean_queue_slug(
    value: &str,
    run_id: &str,
    index: usize,
    used_slugs: &mut HashSet<String>,
) -> String {
    let mut slug = sanitize_daemon_id(value);
    if slug.is_empty() {
        slug = queue_slug(run_id, index);
    }
    while !used_slugs.insert(slug.clone()) {
        slug = queue_slug(run_id, used_slugs.len());
    }
    slug
}

fn queue_track(run_id: &str) -> String {
    format!("queue-{}", sanitize_daemon_id(run_id))
}

fn queue_slug(run_id: &str, index: usize) -> String {
    format!("task-{}-{:02}", sanitize_daemon_id(run_id), index + 1)
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
    if target.is_empty() {
        bail!("terminal target is empty");
    }
    match terminal_input_from_request(payload)? {
        TerminalInput::Paste { text, submit } => send_terminal_paste(target, &text, submit),
        TerminalInput::Literal { text, submit } => send_terminal_literal(target, &text, submit),
        TerminalInput::Key(key) => send_terminal_key(target, key),
    }
}

fn handle_task_chat_target(
    headers: &HeaderMap,
    payload: &TaskChatTargetRequest,
) -> TaskChatResponse {
    match handle_task_chat_target_result(headers, payload) {
        Ok((target, agent_id)) => TaskChatResponse {
            ok: true,
            output: "target ready".to_string(),
            target,
            agent_id,
        },
        Err(err) => TaskChatResponse {
            ok: false,
            output: format!("{err:#}"),
            target: String::new(),
            agent_id: String::new(),
        },
    }
}

fn handle_task_chat_target_result(
    headers: &HeaderMap,
    payload: &TaskChatTargetRequest,
) -> Result<(String, String)> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    ensure_task_chat_target(&payload.task_id)
}

fn handle_task_chat_send(headers: &HeaderMap, payload: &TaskChatSendRequest) -> TaskChatResponse {
    match handle_task_chat_send_result(headers, payload) {
        Ok((target, agent_id)) => TaskChatResponse {
            ok: true,
            output: "sent".to_string(),
            target,
            agent_id,
        },
        Err(err) => TaskChatResponse {
            ok: false,
            output: format!("{err:#}"),
            target: String::new(),
            agent_id: String::new(),
        },
    }
}

fn handle_task_chat_send_result(
    headers: &HeaderMap,
    payload: &TaskChatSendRequest,
) -> Result<(String, String)> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let text = payload.text.trim_end();
    if text.trim().is_empty() {
        bail!("task chat message is empty");
    }
    let (target, agent_id) = if let Some(target) = payload
        .target
        .as_deref()
        .filter(|target| !target.trim().is_empty())
    {
        (clean_terminal_target(target)?, String::new())
    } else {
        ensure_task_chat_target(&payload.task_id)?
    };
    send_task_chat_text(&target, text)?;
    Ok((target, agent_id))
}

fn send_task_chat_text(target: &str, text: &str) -> Result<()> {
    if text.trim_start().starts_with('/') && !text.contains('\n') {
        send_terminal_literal(target, text, true)
    } else {
        send_terminal_paste(target, text, true)
    }
}

fn ensure_task_chat_target(task_id: &str) -> Result<(String, String)> {
    crate::sync_codex_task_records().ok();
    let mut record = task_record_by_id(task_id)?;
    let queue_item = queue_item_for_task_id(&record.id)?;
    if let Some(agent_id) = record
        .agent_id
        .clone()
        .or_else(|| queue_item.as_ref().and_then(|item| item.agent_id.clone()))
    {
        if let Some(target) = active_terminal_target_for_agent(&agent_id) {
            return Ok((target, agent_id));
        }
    }
    if record.status != "closed:blocked" {
        bail!("task has no live chat target");
    }
    let session_id =
        codex_resume_session_id(&record).context("blocked task has no Codex session id")?;
    let agent_command = queue_item
        .as_ref()
        .map(|item| item.agent_command.clone())
        .or_else(|| codex_command_from_metadata(record.metadata_json.as_deref()))
        .unwrap_or_else(|| "c1".to_string());
    if !agents::available_agent_commands()
        .iter()
        .any(|agent| agent.command == agent_command)
    {
        bail!("unknown task chat agent command: {agent_command}");
    }
    let command = format!("{agent_command} resume {}", shell_quote(&session_id));
    let cwd = record
        .cwd
        .as_deref()
        .filter(|path| Path::new(path).is_dir())
        .or(record.repo_root.as_deref().filter(|path| Path::new(path).is_dir()))
        .map(PathBuf::from);
    let request = AgentStartRequest {
        cwd,
        track: "task-chat".to_string(),
        command,
    };
    let agent = start_web_agent(&request)?;
    let target = wait_for_agent_terminal_target(&agent.id)
        .context("task chat terminal did not appear")?;
    record.agent_id = Some(agent.id.clone());
    state::upsert_task_record(&record)?;
    if let Some(item) = queue_item {
        state::set_web_queue_item_agent(&item.run_id, &item.id, &agent.id)?;
    }
    Ok((target, agent.id))
}

fn task_record_by_id(task_id: &str) -> Result<state::TaskRecordRow> {
    let task_id = task_id.trim();
    if task_id.is_empty() || task_id.chars().any(char::is_control) {
        bail!("invalid task id");
    }
    state::get_task_record(task_id)?.with_context(|| format!("unknown task record: {task_id}"))
}

fn active_terminal_target_for_agent(agent_id: &str) -> Option<String> {
    agents::terminal_contexts()
        .ok()?
        .into_iter()
        .find(|context| context.id == agent_id)
        .map(|context| context.target)
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

fn task_transcript_response(task_id: &str) -> TaskTranscriptResponse {
    match task_transcript_result(task_id) {
        Ok(response) => response,
        Err(err) => TaskTranscriptResponse {
            ok: false,
            task_id: task_id.to_string(),
            title: String::new(),
            status: String::new(),
            session_path: None,
            chat_available: false,
            messages: Vec::new(),
            output: format!("{err:#}"),
        },
    }
}

fn task_transcript_result(task_id: &str) -> Result<TaskTranscriptResponse> {
    let task_id = task_id.trim();
    if task_id.is_empty() || task_id.chars().any(char::is_control) {
        bail!("invalid task id");
    }
    crate::sync_codex_task_records().ok();
    let record = state::get_task_record(task_id)?
        .with_context(|| format!("unknown task record: {task_id}"))?;
    let session_path = codex_session_path_from_metadata(record.metadata_json.as_deref())
        .context("task record has no Codex session transcript")?;
    let path = PathBuf::from(&session_path);
    if !is_codex_session_path(&path) {
        bail!("refusing to read non-Codex session path: {}", path.display());
    }
    let messages = codex_transcript_messages(&path)?;
    let chat_available = task_record_chat_available(&record);
    Ok(TaskTranscriptResponse {
        ok: true,
        task_id: record.id,
        title: record.title,
        chat_available,
        status: record.status,
        session_path: Some(session_path),
        messages,
        output: String::new(),
    })
}

fn task_record_chat_available(record: &state::TaskRecordRow) -> bool {
    record.status == "closed:blocked" && codex_resume_session_id(record).is_some()
}

fn codex_session_path_from_metadata(metadata_json: Option<&str>) -> Option<String> {
    let metadata = serde_json::from_str::<Value>(metadata_json?).ok()?;
    metadata
        .get("session_path")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn codex_command_from_metadata(metadata_json: Option<&str>) -> Option<String> {
    let metadata = serde_json::from_str::<Value>(metadata_json?).ok()?;
    metadata
        .get("command")
        .and_then(Value::as_str)
        .and_then(|command| shell_words(command).into_iter().next())
}

fn codex_resume_session_id(record: &state::TaskRecordRow) -> Option<String> {
    let metadata = serde_json::from_str::<Value>(record.metadata_json.as_deref()?).ok()?;
    metadata
        .get("codex_thread_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            metadata
                .get("session_path")
                .and_then(Value::as_str)
                .and_then(codex_thread_id_from_session_path)
        })
}

fn codex_thread_id_from_session_path(path: &str) -> Option<String> {
    let stem = Path::new(path).file_stem()?.to_str()?;
    let id = stem.get(stem.len().saturating_sub(36)..)?;
    if id.len() == 36
        && id.chars().enumerate().all(|(index, ch)| {
            matches!(index, 8 | 13 | 18 | 23) && ch == '-'
                || !matches!(index, 8 | 13 | 18 | 23) && ch.is_ascii_hexdigit()
        })
    {
        Some(id.to_string())
    } else {
        None
    }
}

fn queue_item_for_task_id(task_id: &str) -> Result<Option<state::QueueItemRow>> {
    let (_, items) = state::load_web_queue()?;
    let Some(slug) = task_id.strip_prefix("task/") else {
        return Ok(None);
    };
    Ok(items.into_iter().find(|item| item.slug == slug))
}

fn is_codex_session_path(path: &Path) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let Ok(home) = home.canonicalize() else {
        return false;
    };
    let account_root = home.join(".codex-accounts");
    let default_root = home.join(".codex").join("sessions");
    (path.starts_with(account_root) && path.components().any(|part| part.as_os_str() == "sessions"))
        || path.starts_with(default_root)
}

fn codex_transcript_messages(path: &Path) -> Result<Vec<TaskTranscriptMessage>> {
    let file = File::open(path)
        .with_context(|| format!("failed to open Codex session {}", path.display()))?;
    let mut messages = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.with_context(|| format!("failed to read Codex session {}", path.display()))?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(message) = transcript_message_from_json(&value) {
            push_transcript_message(&mut messages, message);
        }
    }
    Ok(messages)
}

fn transcript_message_from_json(value: &Value) -> Option<TaskTranscriptMessage> {
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match value.get("type").and_then(Value::as_str)? {
        "event_msg" => {
            let payload = value.get("payload")?;
            match payload.get("type").and_then(Value::as_str)? {
                "user_message" => transcript_message(
                    timestamp,
                    "user",
                    payload.get("message").and_then(Value::as_str)?,
                ),
                "agent_message" => transcript_message(
                    timestamp,
                    "assistant",
                    payload.get("message").and_then(Value::as_str)?,
                ),
                _ => None,
            }
        }
        "response_item" => {
            let payload = value.get("payload")?;
            if payload.get("type").and_then(Value::as_str) != Some("message") {
                return None;
            }
            let role = payload.get("role").and_then(Value::as_str)?;
            if !matches!(role, "user" | "assistant") {
                return None;
            }
            transcript_message(timestamp, role, &response_content_text(payload)?)
        }
        _ => None,
    }
}

fn transcript_message(timestamp: String, role: &str, text: &str) -> Option<TaskTranscriptMessage> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(TaskTranscriptMessage {
        timestamp,
        role: role.to_string(),
        text: truncate_chars(text, 30_000),
    })
}

fn response_content_text(payload: &Value) -> Option<String> {
    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|item| {
            let item_type = item.get("type").and_then(Value::as_str)?;
            if !matches!(item_type, "input_text" | "output_text" | "text") {
                return None;
            }
            item.get("text").and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn push_transcript_message(messages: &mut Vec<TaskTranscriptMessage>, message: TaskTranscriptMessage) {
    if messages
        .last()
        .is_some_and(|last| last.role == message.role && last.text == message.text)
    {
        return;
    }
    messages.push(message);
}

fn parse_zellij_target(target: &str) -> Option<(&str, &str)> {
    let rest = target.strip_prefix("zellij:")?;
    rest.split_once(':')
}

enum TerminalInput {
    Paste { text: String, submit: bool },
    Literal { text: String, submit: bool },
    Key(TerminalKey),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum TerminalKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Backspace,
    Delete,
    Escape,
    Tab,
    Home,
    End,
    PageUp,
    PageDown,
}

impl TerminalKey {
    fn tmux(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Enter => "Enter",
            Self::Backspace => "BSpace",
            Self::Delete => "DC",
            Self::Escape => "Escape",
            Self::Tab => "Tab",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
        }
    }

    fn zellij(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Enter => "Enter",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Escape => "Esc",
            Self::Tab => "Tab",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
        }
    }
}

fn terminal_input_from_request(payload: &TerminalSendRequest) -> Result<TerminalInput> {
    let mode = payload.mode.as_deref().unwrap_or("paste").trim();
    match mode {
        "key" => {
            let key = payload
                .key
                .as_deref()
                .context("terminal key is empty")?;
            Ok(TerminalInput::Key(clean_terminal_key(key)?))
        }
        "literal" => {
            let text = terminal_request_text(payload)?;
            Ok(TerminalInput::Literal {
                text,
                submit: payload.submit.unwrap_or(false),
            })
        }
        "paste" | "" => {
            let text = terminal_request_text(payload)?;
            Ok(TerminalInput::Paste {
                text,
                submit: payload.submit.unwrap_or(true),
            })
        }
        _ => bail!("unsupported terminal input mode"),
    }
}

fn terminal_request_text(payload: &TerminalSendRequest) -> Result<String> {
    let text = payload.text.as_deref().unwrap_or_default().trim_end();
    if text.trim().is_empty() {
        bail!("terminal input is empty");
    }
    Ok(text.to_string())
}

fn clean_terminal_key(key: &str) -> Result<TerminalKey> {
    match key.trim() {
        "Up" | "ArrowUp" => Ok(TerminalKey::Up),
        "Down" | "ArrowDown" => Ok(TerminalKey::Down),
        "Left" | "ArrowLeft" => Ok(TerminalKey::Left),
        "Right" | "ArrowRight" => Ok(TerminalKey::Right),
        "Enter" => Ok(TerminalKey::Enter),
        "Backspace" => Ok(TerminalKey::Backspace),
        "Delete" => Ok(TerminalKey::Delete),
        "Escape" | "Esc" => Ok(TerminalKey::Escape),
        "Tab" => Ok(TerminalKey::Tab),
        "Home" => Ok(TerminalKey::Home),
        "End" => Ok(TerminalKey::End),
        "PageUp" => Ok(TerminalKey::PageUp),
        "PageDown" => Ok(TerminalKey::PageDown),
        _ => bail!("unsupported terminal key"),
    }
}

fn send_terminal_paste(target: &str, text: &str, submit: bool) -> Result<()> {
    if let Some((session, pane)) = parse_zellij_target(target) {
        send_zellij_terminal_paste(session, pane, text, submit)
    } else {
        send_tmux_terminal_paste(target, text, submit)
    }
}

fn send_terminal_literal(target: &str, text: &str, submit: bool) -> Result<()> {
    if let Some((session, pane)) = parse_zellij_target(target) {
        send_zellij_terminal_literal(session, pane, text, submit)
    } else {
        send_tmux_terminal_literal(target, text, submit)
    }
}

fn send_terminal_key(target: &str, key: TerminalKey) -> Result<()> {
    if let Some((session, pane)) = parse_zellij_target(target) {
        send_zellij_terminal_key(session, pane, key)
    } else {
        send_tmux_terminal_key(target, key)
    }
}

fn send_tmux_terminal_paste(target: &str, text: &str, submit: bool) -> Result<()> {
    paste_terminal_text(target, text)?;
    if submit {
        thread::sleep(Duration::from_millis(100));
        send_tmux_terminal_key(target, TerminalKey::Enter)?;
    }
    Ok(())
}

fn send_tmux_terminal_literal(target: &str, text: &str, submit: bool) -> Result<()> {
    let status = Command::new("tmux")
        .args(["send-keys", "-t", target, "-l", text])
        .status()
        .context("failed to send literal terminal input through tmux")?;
    if !status.success() {
        bail!("tmux send-keys literal failed with {status}");
    }
    if submit {
        send_tmux_terminal_key(target, TerminalKey::Enter)?;
    }
    Ok(())
}

fn send_tmux_terminal_key(target: &str, key: TerminalKey) -> Result<()> {
    let status = Command::new("tmux")
        .args(["send-keys", "-t", target, key.tmux()])
        .status()
        .context("failed to send terminal key through tmux")?;
    if !status.success() {
        bail!("tmux send-keys failed with {status}");
    }
    Ok(())
}

fn send_zellij_terminal_paste(session: &str, pane: &str, text: &str, submit: bool) -> Result<()> {
    let status = Command::new("zellij")
        .args(["--session", session, "action", "paste", "--pane-id", pane, text])
        .status()
        .context("failed to paste terminal input through zellij")?;
    if !status.success() {
        bail!("zellij action paste failed with {status}");
    }
    if submit {
        thread::sleep(Duration::from_millis(100));
        send_zellij_terminal_key(session, pane, TerminalKey::Enter)?;
    }
    Ok(())
}

fn send_zellij_terminal_literal(session: &str, pane: &str, text: &str, submit: bool) -> Result<()> {
    let status = Command::new("zellij")
        .args([
            "--session",
            session,
            "action",
            "write-chars",
            "--pane-id",
            pane,
            text,
        ])
        .status()
        .context("failed to write terminal input through zellij")?;
    if !status.success() {
        bail!("zellij action write-chars failed with {status}");
    }
    if submit {
        send_zellij_terminal_key(session, pane, TerminalKey::Enter)?;
    }
    Ok(())
}

fn send_zellij_terminal_key(session: &str, pane: &str, key: TerminalKey) -> Result<()> {
    let status = Command::new("zellij")
        .args([
            "--session",
            session,
            "action",
            "send-keys",
            "--pane-id",
            pane,
            key.zellij(),
        ])
        .status()
        .context("failed to send terminal key through zellij")?;
    if !status.success() {
        bail!("zellij action send-keys failed with {status}");
    }
    Ok(())
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
        let request = parse_agent_start(request)?;
        return match start_web_agent(&request) {
            Ok(record) => {
                let output = format!("Started agent:\n{}", agents::snapshot_line(&record));
                Ok(output)
            }
            Err(err) => {
                let output = format!("Failed to start agent: {err:#}");
                Ok(output)
            }
        };
    }
    if text.starts_with('/') {
        bail!("unknown GUI command. Try /status, /repos, /agents, /help, or /agent_start <track> :: <command>");
    }
    history::append("web", "operator", text)?;
    let output = run_meta_agent(text)?;
    history::append("web", "assistant", &output)?;
    Ok(output)
}

fn respond(output: String) -> Result<String> {
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
        "/agent_start [--cwd <repo>] <track> :: <command> - start an agent through Q-COLD",
        "/app - show dashboard context",
        "/help - show this help",
        "",
        "Plain messages start the configured meta-agent command, or `c1 exec --ephemeral --cd <repo> -` by default.",
    ]
    .join("\n")
}

fn parse_agent_start(request: &str) -> Result<AgentStartRequest> {
    let Some((track, command)) = request.split_once("::") else {
        bail!("usage: /agent_start [--cwd <repo>] <track> :: <command>");
    };
    let mut words = shell_words(track);
    let cwd = if words.first().is_some_and(|word| word == "--cwd") {
        if words.len() < 3 {
            bail!("usage: /agent_start [--cwd <repo>] <track> :: <command>");
        }
        words.remove(0);
        Some(PathBuf::from(words.remove(0)))
    } else {
        None
    };
    if words.len() != 1 {
        bail!("usage: /agent_start [--cwd <repo>] <track> :: <command>");
    }
    let track = words.remove(0);
    let command = command.trim();
    if track.is_empty() || command.is_empty() {
        bail!("usage: /agent_start [--cwd <repo>] <track> :: <command>");
    }
    Ok(AgentStartRequest {
        cwd,
        track,
        command: command.to_string(),
    })
}

fn start_web_agent(request: &AgentStartRequest) -> Result<agents::AgentRecord> {
    if let Some(cwd) = request.cwd.clone() {
        agents::start_terminal_shell_agent_in_cwd(&request.track, &request.command, cwd)
    } else {
        agents::start_terminal_shell_agent(&request.track, &request.command)
    }
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
    let cwd = repository::current_or_active_root()?;
    Ok(default_meta_agent_command(&cwd))
}

fn default_meta_agent_command(cwd: &Path) -> String {
    format!(
        "c1 exec --ephemeral --cd {} -",
        shell_quote(&cwd.display().to_string())
    )
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
        agents: SnapshotBlock::capture("running managed agents", agents::running_snapshot),
        task_records: task_record_snapshot(&root),
        queue_task_records: all_task_record_snapshot(),
        queue: queue_snapshot(),
        host_agents: discover_host_agents(),
        terminals: discover_terminal_sessions(),
        available_agents: AvailableAgentSnapshot::discover(),
        commands: CommandTemplates {
            agent_start_template: agent_start_template(&root),
        },
    }
}

fn agent_start_template(root: &str) -> String {
    format!(
        "/agent_start --cwd {cwd} <track> :: codex exec \"Use the launched host-side agent workspace as your home base for {root}; do not enter a devcontainer from $QCOLD_AGENT_WORKTREE. Start managed task <slug> with cargo qcold task open <slug>, enter that managed task worktree and its devcontainer if the task flow provides one, reread AGENTS.md and task logs, then do: <task>. Drive the task to terminal closeout unless blocked. After closeout, cd back to $QCOLD_AGENT_WORKTREE before starting a new chat or task.\"",
        cwd = shell_quote(root),
    )
}

fn task_record_snapshot(repo_root: &str) -> TaskRecordSnapshot {
    let sync_error = crate::sync_codex_task_records().err().map(|err| format!("{err:#}"));
    match state::load_task_records_for_repo(repo_root, None, 250) {
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

fn all_task_record_snapshot() -> TaskRecordSnapshot {
    let sync_error = crate::sync_codex_task_records().err().map(|err| format!("{err:#}"));
    match state::load_task_records(None, 500) {
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

fn queue_snapshot() -> QueueSnapshot {
    let reconcile_error = reconcile_stale_web_queue_run()
        .err()
        .map(|err| format!("{err:#}"));
    match state::load_web_queue() {
        Ok((run, records)) => QueueSnapshot {
            count: records.len(),
            running: run.as_ref().is_some_and(|run| {
                matches!(run.status.as_str(), "running" | "waiting" | "starting" | "stopping")
            }),
            run,
            records,
            error: reconcile_error,
        },
        Err(err) => QueueSnapshot {
            count: 0,
            running: false,
            run: None,
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

#[derive(Clone)]
struct AgentLabelRecord {
    label: String,
    track: String,
    target: String,
}

fn agent_labels_by_id() -> HashMap<String, AgentLabelRecord> {
    let metadata = terminal_metadata_by_target();
    agents::terminal_contexts()
        .unwrap_or_default()
        .into_iter()
        .map(|context| {
            let label = metadata
                .get(&context.target)
                .and_then(|metadata| metadata.name.as_deref())
                .filter(|name| !name.trim().is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| generated_agent_label(&context));
            (
                context.id.clone(),
                AgentLabelRecord {
                    label,
                    track: context.track,
                    target: context.target,
                },
            )
        })
        .collect()
}

fn generated_agent_label(context: &agents::TerminalAgentContext) -> String {
    let suffix = short_terminal_id(&context.id);
    if let Some(summary) = terminal_command_summary(&context.command) {
        return format!("{}: {} #{suffix}", context.track, summary);
    }
    format!("{} #{suffix}", context.track)
}

fn apply_terminal_details(
    pane: &mut TerminalPane,
    context: Option<&agents::TerminalAgentContext>,
    metadata: Option<&state::TerminalMetadataRow>,
) {
    pane.agent_id = context
        .map(|context| context.id.clone())
        .unwrap_or_default();
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
        return generated_agent_label(context);
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
    match repository::current_or_active() {
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

fn base36_time_id() -> String {
    let mut value = unix_now();
    if value == 0 {
        return "0".to_string();
    }
    let mut chars = Vec::new();
    while value > 0 {
        let digit = (value % 36) as u8;
        chars.push(match digit {
            0..=9 => char::from(b'0' + digit),
            _ => char::from(b'a' + digit - 10),
        });
        value /= 36;
    }
    chars.into_iter().rev().collect()
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
    queue_task_records: TaskRecordSnapshot,
    queue: QueueSnapshot,
    host_agents: HostAgentSnapshot,
    terminals: TerminalSnapshot,
    available_agents: AvailableAgentSnapshot,
    commands: CommandTemplates,
}

#[derive(Serialize)]
struct EventSnapshot {
    state: DashboardState,
    history: Vec<history::HistoryEntry>,
}

#[derive(Serialize)]
struct QueueSnapshot {
    count: usize,
    running: bool,
    run: Option<state::QueueRunRow>,
    records: Vec<state::QueueItemRow>,
    error: Option<String>,
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
        let agent_labels = agent_labels_by_id();
        let records = rows
            .into_iter()
            .map(|row| WebTaskRecord::from_row(row, &agent_labels))
            .collect::<Vec<_>>();
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
    agent_label: Option<String>,
    agent_track: Option<String>,
    agent_target: Option<String>,
    kind: Option<String>,
    codex_thread_id: Option<String>,
    session_path: Option<String>,
    token_usage: Option<TaskTokenUsage>,
    token_efficiency: Option<TaskTokenEfficiency>,
}

impl WebTaskRecord {
    fn from_row(
        row: state::TaskRecordRow,
        agent_labels: &HashMap<String, AgentLabelRecord>,
    ) -> Self {
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
        let agent = row
            .agent_id
            .as_deref()
            .and_then(|agent_id| agent_labels.get(agent_id));
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
            agent_label: agent.map(|agent| agent.label.clone()),
            agent_track: agent.map(|agent| agent.track.clone()),
            agent_target: agent.map(|agent| agent.target.clone()),
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

struct AgentStartRequest {
    cwd: Option<PathBuf>,
    track: String,
    command: String,
}

#[derive(Serialize)]
struct AvailableAgentSnapshot {
    count: usize,
    records: Vec<agents::AvailableAgentCommand>,
}

impl AvailableAgentSnapshot {
    fn discover() -> Self {
        let records = agents::available_agent_commands();
        Self {
            count: records.len(),
            records,
        }
    }
}

#[derive(Clone)]
struct AgentLimitCache {
    generated_at_unix: u64,
    records: Vec<AgentLimitRecord>,
}

#[derive(Serialize)]
struct AgentLimitSnapshot {
    generated_at_unix: u64,
    cached: bool,
    count: usize,
    records: Vec<AgentLimitRecord>,
}

#[derive(Clone, Serialize)]
struct AgentLimitRecord {
    command: String,
    account: String,
    status_command: String,
    state: String,
    summary: String,
    detail: String,
    checked_at_unix: u64,
    attempts: usize,
}

fn agent_limit_snapshot(refresh: bool) -> AgentLimitSnapshot {
    let now = unix_now();
    let cache = AGENT_LIMIT_CACHE.get_or_init(|| Mutex::new(None));
    if !refresh {
        if let Some(cached) = cache.lock().ok().and_then(|guard| guard.clone()) {
            if now.saturating_sub(cached.generated_at_unix) < AGENT_LIMIT_CACHE_TTL {
                return AgentLimitSnapshot {
                    generated_at_unix: cached.generated_at_unix,
                    cached: true,
                    count: cached.records.len(),
                    records: cached.records,
                };
            }
        }
    }

    let records = probe_agent_limits();
    let snapshot = AgentLimitSnapshot {
        generated_at_unix: now,
        cached: false,
        count: records.len(),
        records: records.clone(),
    };
    if let Ok(mut cached) = cache.lock() {
        *cached = Some(AgentLimitCache {
            generated_at_unix: now,
            records,
        });
    }
    snapshot
}

fn probe_agent_limits() -> Vec<AgentLimitRecord> {
    let agents = agents::available_agent_commands();
    let mut probes = BTreeMap::<String, agents::AvailableAgentCommand>::new();
    for agent in &agents {
        probes
            .entry(agent.account.clone())
            .or_insert_with(|| agent.clone());
    }
    let handles = probes
        .into_values()
        .map(|agent| thread::spawn(move || (agent.account.clone(), probe_agent_limit(&agent))))
        .collect::<Vec<_>>();
    let mut by_account = BTreeMap::new();
    for handle in handles {
        if let Ok((account, record)) = handle.join() {
            by_account.insert(account, record);
        }
    }
    agents
        .into_iter()
        .map(|agent| {
            let Some(record) = by_account.get(&agent.account) else {
                return unchecked_agent_limit(&agent, "status probe did not return");
            };
            AgentLimitRecord {
                command: agent.command,
                account: agent.account,
                status_command: agent.status_command,
                state: record.state.clone(),
                summary: record.summary.clone(),
                detail: record.detail.clone(),
                checked_at_unix: record.checked_at_unix,
                attempts: record.attempts,
            }
        })
        .collect()
}

fn probe_agent_limit(agent: &agents::AvailableAgentCommand) -> AgentLimitRecord {
    if !agent_auth_file(&agent.account).is_file() {
        return AgentLimitRecord {
            command: agent.command.clone(),
            account: agent.account.clone(),
            status_command: agent.status_command.clone(),
            state: "unauthenticated".to_string(),
            summary: "no auth.json".to_string(),
            detail: format!("missing {}", agent_auth_file(&agent.account).display()),
            checked_at_unix: unix_now(),
            attempts: 0,
        };
    }
    let mut last = None;
    for attempt in 1..=AGENT_LIMIT_STATUS_ATTEMPTS {
        let output = run_agent_status_probe(&agent.status_command);
        let record = classify_agent_status_output(agent, attempt, output);
        if record.state != "retry" {
            return record;
        }
        last = Some(record);
    }
    last.unwrap_or_else(|| unchecked_agent_limit(agent, "status probe was not attempted"))
}

fn run_agent_status_probe(command: &str) -> io::Result<std::process::Output> {
    Command::new("timeout")
        .arg(format!("{AGENT_LIMIT_STATUS_TIMEOUT}s"))
        .arg(command)
        .arg("exec")
        .arg("status")
        .env("QCOLD_AGENT_MANAGED_WORKTREE", "0")
        .stdin(Stdio::null())
        .output()
}

fn classify_agent_status_output(
    agent: &agents::AvailableAgentCommand,
    attempt: usize,
    output: io::Result<std::process::Output>,
) -> AgentLimitRecord {
    let checked_at_unix = unix_now();
    let output = match output {
        Ok(output) => output,
        Err(err) => {
            return AgentLimitRecord {
                command: agent.command.clone(),
                account: agent.account.clone(),
                status_command: agent.status_command.clone(),
                state: "error".to_string(),
                summary: "status probe failed to start".to_string(),
                detail: err.to_string(),
                checked_at_unix,
                attempts: attempt,
            };
        }
    };
    let text = compact_probe_output(&output);
    let lower = text.to_lowercase();
    let code = output.status.code().unwrap_or_default();
    let timed_out = code == 124;
    let limited = lower.contains("usage limit")
        || lower.contains("rate limit")
        || lower.contains("quota")
        || (lower.contains("limit") && (lower.contains("reached") || lower.contains("exceeded")));
    let transient = lower.contains("try again")
        || lower.contains("temporar")
        || lower.contains("429")
        || timed_out;
    let state = if limited {
        "limited"
    } else if timed_out {
        "timeout"
    } else if output.status.success() {
        "ok"
    } else if transient && attempt < AGENT_LIMIT_STATUS_ATTEMPTS {
        "retry"
    } else {
        "error"
    };
    let summary = if limited {
        extract_relevant_status_line(&text).unwrap_or_else(|| "limit reached".to_string())
    } else if timed_out {
        format!("status probe timed out after {AGENT_LIMIT_STATUS_TIMEOUT}s")
    } else if output.status.success() {
        extract_relevant_status_line(&text).unwrap_or_else(|| "status probe completed".to_string())
    } else {
        extract_relevant_status_line(&text)
            .unwrap_or_else(|| format!("status probe exited with {}", output.status))
    };
    AgentLimitRecord {
        command: agent.command.clone(),
        account: agent.account.clone(),
        status_command: agent.status_command.clone(),
        state: state.to_string(),
        summary,
        detail: truncate_chars(&text, 600),
        checked_at_unix,
        attempts: attempt,
    }
}

fn compact_probe_output(output: &std::process::Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    strip_ansi(&text)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(80)
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_relevant_status_line(text: &str) -> Option<String> {
    let keywords = [
        "limit",
        "remaining",
        "reset",
        "quota",
        "rate",
        "usage",
        "try again",
        "error",
    ];
    text.lines()
        .find(|line| {
            let lower = line.to_lowercase();
            keywords.iter().any(|keyword| lower.contains(keyword))
        })
        .map(|line| truncate_chars(line.trim(), 160))
}

fn strip_ansi(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            result.push(ch);
            continue;
        }
        while let Some(next) = chars.next() {
            if next.is_ascii_alphabetic() || matches!(next, '~' | '\\') {
                break;
            }
        }
    }
    result
}

fn unchecked_agent_limit(
    agent: &agents::AvailableAgentCommand,
    summary: impl Into<String>,
) -> AgentLimitRecord {
    AgentLimitRecord {
        command: agent.command.clone(),
        account: agent.account.clone(),
        status_command: agent.status_command.clone(),
        state: "unknown".to_string(),
        summary: summary.into(),
        detail: String::new(),
        checked_at_unix: unix_now(),
        attempts: 0,
    }
}

fn agent_auth_file(account: &str) -> PathBuf {
    let home = env::var("HOME").unwrap_or_default();
    if account == "default" {
        return PathBuf::from(home).join(".codex/auth.json");
    }
    PathBuf::from(home)
        .join(".codex-accounts")
        .join(account)
        .join("auth.json")
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
    agent_id: String,
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
            agent_id: String::new(),
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
struct AgentLimitQuery {
    refresh: Option<String>,
}

#[derive(Deserialize)]
struct TaskTranscriptQuery {
    id: String,
}

#[derive(Serialize)]
struct TaskTranscriptResponse {
    ok: bool,
    task_id: String,
    title: String,
    status: String,
    session_path: Option<String>,
    chat_available: bool,
    messages: Vec<TaskTranscriptMessage>,
    output: String,
}

#[derive(Serialize)]
struct TaskTranscriptMessage {
    timestamp: String,
    role: String,
    text: String,
}

#[derive(Deserialize)]
struct QueueRunRequest {
    run_id: Option<String>,
    selected_agent_command: String,
    selected_repo_root: Option<String>,
    selected_repo_name: Option<String>,
    items: Vec<QueueRunItemRequest>,
}

#[derive(Deserialize)]
struct QueueRunItemRequest {
    id: Option<String>,
    prompt: String,
    slug: Option<String>,
    repo_root: Option<String>,
    repo_name: Option<String>,
    agent_command: Option<String>,
}

#[derive(Deserialize)]
struct QueueAppendRequest {
    run_id: String,
    items: Vec<QueueRunItemRequest>,
}

#[derive(Deserialize)]
struct QueueRemoveRequest {
    run_id: String,
    item_id: String,
    task_id: Option<String>,
    agent_id: Option<String>,
}

#[derive(Deserialize)]
struct QueueClearRequest {
    run_id: Option<String>,
}

#[derive(Deserialize)]
struct ChatRequest {
    text: String,
}

#[derive(Deserialize)]
struct TaskChatTargetRequest {
    task_id: String,
}

#[derive(Deserialize)]
struct TaskChatSendRequest {
    task_id: String,
    target: Option<String>,
    text: String,
}

#[derive(Deserialize)]
struct TerminalSendRequest {
    target: String,
    text: Option<String>,
    mode: Option<String>,
    key: Option<String>,
    submit: Option<bool>,
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

#[derive(Serialize)]
struct TaskChatResponse {
    ok: bool,
    output: String,
    target: String,
    agent_id: String,
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
    fn default_meta_agent_command_uses_c1_exec() {
        assert_eq!(
            default_meta_agent_command(Path::new("/workspace/repo")),
            "c1 exec --ephemeral --cd '/workspace/repo' -"
        );
    }

    #[test]
    fn terminal_key_mapping_supports_history_navigation() {
        let key = clean_terminal_key("ArrowUp").unwrap();

        assert_eq!(key, TerminalKey::Up);
        assert_eq!(key.tmux(), "Up");
        assert_eq!(key.zellij(), "Up");
        assert!(clean_terminal_key("$(touch /tmp/nope)").is_err());
    }

    #[test]
    fn terminal_send_request_supports_literal_slash_commands() {
        let request = TerminalSendRequest {
            target: "main:0.1".to_string(),
            text: Some("/new".to_string()),
            mode: Some("literal".to_string()),
            key: None,
            submit: Some(true),
        };

        match terminal_input_from_request(&request).unwrap() {
            TerminalInput::Literal { text, submit } => {
                assert_eq!(text, "/new");
                assert!(submit);
            }
            _ => panic!("expected literal input"),
        }
    }

    #[test]
    fn agent_start_template_keeps_agent_workspace_host_side() {
        let template = agent_start_template("/workspace/repo");
        assert!(template.contains("/agent_start --cwd '/workspace/repo' <track>"));
        assert!(template.contains("host-side agent workspace"));
        assert!(template.contains("do not enter a devcontainer from $QCOLD_AGENT_WORKTREE"));
        assert!(template.contains("enter that managed task worktree and its devcontainer"));
    }

    #[test]
    fn agent_start_parser_accepts_cwd_prefix() {
        let request = parse_agent_start("--cwd '/workspace/repo with space' queue :: c1 exec 'do work'").unwrap();

        assert_eq!(request.cwd.as_deref(), Some(Path::new("/workspace/repo with space")));
        assert_eq!(request.track, "queue");
        assert_eq!(request.command, "c1 exec 'do work'");
    }

    #[test]
    fn queue_task_instruction_starts_managed_task() {
        let item = state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            prompt: "do focused work".to_string(),
            slug: "task-run-01".to_string(),
            repo_root: Some("/workspace/repo".to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };

        let instruction = queue_task_instruction(&item);
        assert!(instruction.contains("home base for /workspace/repo"));
        assert!(instruction.contains("cargo qcold task open task-run-01"));
        assert!(instruction.contains("then do: do focused work"));
        assert!(instruction.contains("Drive the task to terminal closeout unless blocked"));
    }

    #[test]
    fn queue_terminal_scope_uses_managed_task_slug() {
        let item = state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            prompt: "do focused work".to_string(),
            slug: "task-mozgpaqk-03".to_string(),
            repo_root: Some("/workspace/repo".to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };

        assert_eq!(queue_terminal_scope(&item), "task/task-mozgpaqk-03");
    }

    #[test]
    fn queue_slug_deduplicates_with_run_prefix() {
        let mut used = HashSet::new();
        assert_eq!(
            clean_queue_slug("task-run-01", "run", 0, &mut used),
            "task-run-01"
        );
        assert_eq!(
            clean_queue_slug("task-run-01", "run", 1, &mut used),
            "task-run-02"
        );
    }

    #[test]
    fn queue_item_outcome_distinguishes_retryable_launch_failures() {
        match QueueItemOutcome::retryable_failure("terminal setup failed") {
            QueueItemOutcome::Failed { message, retryable } => {
                assert_eq!(message, "terminal setup failed");
                assert!(retryable);
            }
            _ => panic!("expected failed outcome"),
        }

        match QueueItemOutcome::failed("agent exited before task closeout") {
            QueueItemOutcome::Failed { message, retryable } => {
                assert_eq!(message, "agent exited before task closeout");
                assert!(!retryable);
            }
            _ => panic!("expected failed outcome"),
        }
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

    #[test]
    fn codex_transcript_messages_include_user_and_agent_text() {
        let temp = tempdir().unwrap();
        let session = temp.path().join("session.jsonl");
        fs::write(
            &session,
            concat!(
                "{\"timestamp\":\"2026-05-10T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"fix queue\",\"images\":[]}}\n",
                "{\"timestamp\":\"2026-05-10T00:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"queue fixed\"}]}}\n",
                "{\"timestamp\":\"2026-05-10T00:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":null}}\n"
            ),
        )
        .unwrap();

        let messages = codex_transcript_messages(&session).unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].text, "fix queue");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].text, "queue fixed");
    }
}
