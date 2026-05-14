use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::env;
use std::fmt::Write as FmtWrite;
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
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse,
    },
    routing::{get, post},
    Router,
};
use clap::Args;
use futures_util::stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{agents, prompt, repository, state, status};

const DAEMON_STARTUP_CHECKS: usize = 10;
const DAEMON_STARTUP_CHECK_INTERVAL: Duration = Duration::from_millis(100);
const DAEMON_SHUTDOWN_CHECKS: usize = 50;
const DAEMON_SHUTDOWN_CHECK_INTERVAL: Duration = Duration::from_millis(100);
const AGENT_LIMIT_CACHE_TTL: u64 = 600;
const AGENT_LIMIT_STATUS_ATTEMPTS: usize = 2;
const AGENT_LIMIT_STATUS_TIMEOUT: u64 = 20;
const DASHBOARD_STATE_CACHE_TTL: u64 = 2;
const DASHBOARD_STATE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const WEB_QUEUE_RETRY_DELAYS: [u64; 3] = [60, 300, 600];
static AGENT_LIMIT_CACHE: OnceLock<Mutex<Option<AgentLimitCache>>> = OnceLock::new();
static DASHBOARD_STATE_CACHE: OnceLock<Mutex<Option<DashboardStateCache>>> = OnceLock::new();
static DASHBOARD_STATE_REFRESHING: OnceLock<Mutex<bool>> = OnceLock::new();
static DASHBOARD_STATE_REFRESHER: OnceLock<()> = OnceLock::new();
static WEB_QUEUE_WORKERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static WEB_QUEUE_ITEM_WORKERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[derive(Args, Clone)]
pub struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: String,
    #[arg(long, help = "Run the local web dashboard as a persistent Q-COLD daemon")]
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
            "Mini App daemon pid file {} points at pid {pid}, but that process is not the \
             Q-COLD web daemon for {listen}",
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

pub(crate) fn stop_daemon_for_listen(listen: &str) -> Result<()> {
    let paths = WebappDaemonPaths::new(listen)?;
    replace_existing_daemon(&paths, listen)
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

pub(crate) fn dashboard_state_for_tui() -> DashboardState {
    dashboard_state()
}

pub(crate) fn queue_run_for_tui(payload: QueueRunRequest) -> TerminalSendResponse {
    let headers = tui_write_headers();
    let response = handle_queue_run(&headers, payload);
    refresh_dashboard_state_after_mutation(response.ok);
    response
}

pub(crate) fn queue_append_for_tui(payload: QueueAppendRequest) -> TerminalSendResponse {
    let headers = tui_write_headers();
    let response = handle_queue_append(&headers, payload);
    refresh_dashboard_state_after_mutation(response.ok);
    response
}

pub(crate) fn queue_stop_for_tui() -> TerminalSendResponse {
    let headers = tui_write_headers();
    let response = handle_queue_stop(&headers);
    refresh_dashboard_state_after_mutation(response.ok);
    response
}

pub(crate) fn queue_continue_for_tui(run_id: String) -> TerminalSendResponse {
    let headers = tui_write_headers();
    let payload = QueueContinueRequest { run_id };
    let response = handle_queue_continue(&headers, &payload);
    refresh_dashboard_state_after_mutation(response.ok);
    response
}

pub(crate) fn queue_remove_for_tui(payload: &QueueRemoveRequest) -> TerminalSendResponse {
    let headers = tui_write_headers();
    let response = handle_queue_remove(&headers, payload);
    refresh_dashboard_state_after_mutation(response.ok);
    response
}

pub(crate) fn queue_clear_for_tui(run_id: Option<String>) -> TerminalSendResponse {
    let headers = tui_write_headers();
    let payload = QueueClearRequest { run_id };
    let response = handle_queue_clear(&headers, &payload);
    refresh_dashboard_state_after_mutation(response.ok);
    response
}

pub(crate) fn terminal_send_for_tui(payload: &TerminalSendRequest) -> TerminalSendResponse {
    let headers = tui_write_headers();
    let response = handle_terminal_send(&headers, payload);
    refresh_dashboard_state_after_mutation(response.ok);
    response
}

fn tui_write_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(token) = env::var("QCOLD_WEBAPP_WRITE_TOKEN") {
        if let Ok(value) = HeaderValue::from_str(token.trim()) {
            headers.insert("x-qcold-write-token", value);
        }
    }
    headers
}

async fn serve_async(args: &ServeArgs) -> Result<()> {
    refresh_dashboard_state_cache();
    start_dashboard_state_cache_refresher();
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
        .route("/favicon.ico", get(favicon_svg))
        .route("/favicon.svg", get(favicon_svg))
        .route("/assets/app.css", get(app_css))
        .route("/assets/queue.css", get(queue_css))
        .route("/assets/app.js", get(app_js))
        .route("/api/state", get(api_state))
        .route("/api/agent-limits", get(api_agent_limits))
        .route("/api/task-transcript", get(api_task_transcript))
        .route("/api/task-chat/target", post(api_task_chat_target))
        .route("/api/task-chat/send", post(api_task_chat_send))
        .route("/api/queue/run", post(api_queue_run))
        .route("/api/queue/append", post(api_queue_append))
        .route("/api/queue/update", post(api_queue_update))
        .route("/api/queue/remove", post(api_queue_remove))
        .route("/api/queue/clear", post(api_queue_clear))
        .route("/api/queue/stop", post(api_queue_stop))
        .route("/api/queue/continue", post(api_queue_continue))
        .route("/api/terminal/send", post(api_terminal_send))
        .route("/api/terminal/metadata", post(api_terminal_metadata))
        .route("/api/events", get(api_events))
        .route("/healthz", get(healthz))
}

async fn index() -> impl IntoResponse {
    no_store(Html(INDEX_HTML))
}

async fn app_css() -> impl IntoResponse {
    no_store(([(CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS))
}

async fn queue_css() -> impl IntoResponse {
    no_store(([(CONTENT_TYPE, "text/css; charset=utf-8")], QUEUE_CSS))
}

async fn app_js() -> impl IntoResponse {
    no_store((
        [(CONTENT_TYPE, "application/javascript; charset=utf-8")],
        APP_JS,
    ))
}

async fn favicon_svg() -> impl IntoResponse {
    no_store(([(CONTENT_TYPE, "image/svg+xml; charset=utf-8")], FAVICON_SVG))
}

async fn api_state() -> impl IntoResponse {
    no_store((
        [(CONTENT_TYPE, "application/json; charset=utf-8")],
        cached_dashboard_state_json(),
    ))
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
    refresh_dashboard_state_after_mutation(response.ok);
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
    refresh_dashboard_state_after_mutation(response.ok);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_queue_update(
    headers: HeaderMap,
    Json(payload): Json<QueueUpdateRequest>,
) -> impl IntoResponse {
    let response = handle_queue_update(&headers, payload);
    refresh_dashboard_state_after_mutation(response.ok);
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
    refresh_dashboard_state_after_mutation(response.ok);
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
    refresh_dashboard_state_after_mutation(response.ok);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_queue_stop(headers: HeaderMap) -> impl IntoResponse {
    let response = handle_queue_stop(&headers);
    refresh_dashboard_state_after_mutation(response.ok);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_queue_continue(
    headers: HeaderMap,
    Json(payload): Json<QueueContinueRequest>,
) -> impl IntoResponse {
    let response = handle_queue_continue(&headers, &payload);
    refresh_dashboard_state_after_mutation(response.ok);
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
    refresh_dashboard_state_after_mutation(response.ok);
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
    refresh_dashboard_state_after_mutation(response.ok);
    let status = if response.ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    no_store((status, Json(response)))
}

async fn api_events() -> impl IntoResponse {
    let events = stream::unfold(true, |first| async move {
        if !first {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        let event = Event::default()
            .event("snapshot")
            .data(cached_event_snapshot_json());
        Some((Ok::<Event, Infallible>(event), false))
    });
    no_store(Sse::new(events).keep_alive(KeepAlive::default()))
}

async fn api_terminal_send(
    headers: HeaderMap,
    Json(payload): Json<TerminalSendRequest>,
) -> impl IntoResponse {
    let response = handle_terminal_send(&headers, &payload);
    refresh_dashboard_state_after_mutation(response.ok);
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
    refresh_dashboard_state_after_mutation(response.ok);
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

include!("webapp/queue_api.rs");
include!("webapp/queue_worker.rs");
include!("webapp/queue_worker_reconcile.rs");
include!("webapp/queue_worker_taskflow.rs");
include!("webapp/terminal_chat.rs");
include!("webapp/queue_worker_terminal.rs");
include!("webapp/queue_worker_ids.rs");
include!("webapp/snapshot.rs");
include!("webapp/models_assets.rs");

include!("webapp/tests.rs");
include!("webapp/tests_assets.rs");
include!("webapp/tests_queue_live_edit.rs");
include!("webapp/tests_queue_prompt.rs");
include!("webapp/tests_queue_reconcile.rs");
include!("webapp/tests_queue_taskflow.rs");
include!("webapp/tests_terminal_send.rs");
