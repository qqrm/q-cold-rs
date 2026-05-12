#[derive(Serialize)]
pub(crate) struct DashboardState {
    pub(crate) generated_at_unix: u64,
    pub(crate) daemon_cwd: String,
    pub(crate) repository: RepositoryContext,
    pub(crate) repositories: Vec<RepositoryContext>,
    pub(crate) status: SnapshotBlock,
    pub(crate) agents: SnapshotBlock,
    pub(crate) task_records: TaskRecordSnapshot,
    pub(crate) queue_task_records: TaskRecordSnapshot,
    pub(crate) queue: QueueSnapshot,
    pub(crate) host_agents: HostAgentSnapshot,
    pub(crate) terminals: TerminalSnapshot,
    pub(crate) available_agents: AvailableAgentSnapshot,
    pub(crate) commands: CommandTemplates,
}

#[derive(Serialize)]
struct EventSnapshot {
    state: DashboardState,
}

#[derive(Serialize)]
pub(crate) struct QueueSnapshot {
    pub(crate) count: usize,
    pub(crate) running: bool,
    pub(crate) run: Option<state::QueueRunRow>,
    pub(crate) records: Vec<state::QueueItemRow>,
    pub(crate) error: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct TaskRecordSnapshot {
    pub(crate) count: usize,
    pub(crate) open: usize,
    pub(crate) closed: usize,
    pub(crate) failed: usize,
    pub(crate) total_displayed_tokens: u64,
    pub(crate) total_output_tokens: u64,
    pub(crate) total_reasoning_tokens: u64,
    pub(crate) total_tool_output_tokens: u64,
    pub(crate) total_large_tool_outputs: u64,
    pub(crate) records: Vec<WebTaskRecord>,
    pub(crate) error: Option<String>,
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
            .filter(|record| matches!(record.status.as_str(), "open" | "paused"))
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
pub(crate) struct WebTaskRecord {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) sequence: Option<u64>,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) status: String,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
    pub(crate) repo_root: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) agent_track: Option<String>,
    pub(crate) agent_target: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) codex_thread_id: Option<String>,
    pub(crate) session_path: Option<String>,
    pub(crate) token_usage: Option<TaskTokenUsage>,
    pub(crate) token_efficiency: Option<TaskTokenEfficiency>,
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

#[allow(
    clippy::struct_field_names,
    reason = "serialized token telemetry field names mirror task metadata keys"
)]
#[derive(Clone, Serialize)]
pub(crate) struct TaskTokenUsage {
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
pub(crate) struct TaskTokenEfficiency {
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
pub(crate) struct RepositoryContext {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) root: String,
    pub(crate) adapter: String,
    pub(crate) active: bool,
    pub(crate) branch: String,
    pub(crate) webapp_url: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct SnapshotBlock {
    pub(crate) label: &'static str,
    pub(crate) ok: bool,
    pub(crate) text: String,
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
pub(crate) struct CommandTemplates {
    pub(crate) agent_start_template: String,
}

struct AgentStartRequest {
    id: Option<String>,
    cwd: Option<PathBuf>,
    track: String,
    command: String,
}

#[derive(Serialize)]
pub(crate) struct AvailableAgentSnapshot {
    pub(crate) count: usize,
    pub(crate) records: Vec<agents::AvailableAgentCommand>,
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
        .arg("--version")
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
        format!("readiness probe timed out after {AGENT_LIMIT_STATUS_TIMEOUT}s")
    } else if output.status.success() {
        extract_relevant_status_line(&text)
            .unwrap_or_else(|| "readiness probe completed".to_string())
    } else {
        extract_relevant_status_line(&text)
            .unwrap_or_else(|| format!("readiness probe exited with {}", output.status))
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
        for next in chars.by_ref() {
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
pub(crate) struct HostAgentSnapshot {
    pub(crate) count: usize,
    pub(crate) records: Vec<HostAgentRecord>,
}

#[derive(Serialize)]
pub(crate) struct HostAgentRecord {
    pub(crate) pid: u32,
    pub(crate) kind: String,
    pub(crate) cwd: String,
    pub(crate) command: String,
}

#[derive(Default, Serialize)]
pub(crate) struct TerminalSnapshot {
    pub(crate) count: usize,
    pub(crate) records: Vec<TerminalPane>,
}

#[derive(Serialize)]
pub(crate) struct TerminalPane {
    pub(crate) target: String,
    pub(crate) session: String,
    pub(crate) pane: String,
    pub(crate) pid: u32,
    pub(crate) agent_id: String,
    pub(crate) command: String,
    pub(crate) cwd: String,
    pub(crate) label: String,
    pub(crate) generated_label: String,
    pub(crate) name: String,
    pub(crate) scope: String,
    pub(crate) output: String,
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
pub(crate) struct QueueRunRequest {
    pub(crate) run_id: Option<String>,
    pub(crate) execution_mode: Option<String>,
    pub(crate) selected_agent_command: String,
    pub(crate) selected_repo_root: Option<String>,
    pub(crate) selected_repo_name: Option<String>,
    pub(crate) items: Vec<QueueRunItemRequest>,
}

#[derive(Deserialize)]
pub(crate) struct QueueRunItemRequest {
    pub(crate) id: Option<String>,
    pub(crate) prompt: String,
    pub(crate) slug: Option<String>,
    pub(crate) depends_on: Option<Vec<String>>,
    pub(crate) repo_root: Option<String>,
    pub(crate) repo_name: Option<String>,
    pub(crate) agent_command: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct QueueAppendRequest {
    pub(crate) run_id: String,
    pub(crate) items: Vec<QueueRunItemRequest>,
}

#[derive(Deserialize)]
struct QueueUpdateRequest {
    run_id: String,
    items: Vec<QueueUpdateItemRequest>,
}

#[derive(Deserialize)]
struct QueueUpdateItemRequest {
    id: String,
    prompt: String,
    position: Option<i64>,
    depends_on: Option<Vec<String>>,
    repo_root: Option<String>,
    repo_name: Option<String>,
    agent_command: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct QueueContinueRequest {
    pub(crate) run_id: String,
}

#[allow(
    clippy::struct_field_names,
    reason = "request payload names match the dashboard API contract"
)]
#[derive(Deserialize)]
pub(crate) struct QueueRemoveRequest {
    pub(crate) run_id: String,
    pub(crate) item_id: String,
    pub(crate) task_id: Option<String>,
    pub(crate) agent_id: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct QueueClearRequest {
    pub(crate) run_id: Option<String>,
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
pub(crate) struct TerminalSendRequest {
    pub(crate) target: String,
    pub(crate) text: Option<String>,
    pub(crate) mode: Option<String>,
    pub(crate) key: Option<String>,
    pub(crate) submit: Option<bool>,
}

#[derive(Deserialize)]
struct TerminalMetadataRequest {
    target: String,
    name: Option<String>,
    scope: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct TerminalSendResponse {
    pub(crate) ok: bool,
    pub(crate) output: String,
}

#[derive(Serialize)]
struct TaskChatResponse {
    ok: bool,
    output: String,
    target: String,
    agent_id: String,
}

const INDEX_HTML: &str = include_str!("../webapp_assets/index.html");
const APP_CSS: &str = include_str!("../webapp_assets/app.css");
const APP_JS: &str = concat!(
    include_str!("../webapp_assets/app/init_parse.js"),
    include_str!("../webapp_assets/app/queue.js"),
    include_str!("../webapp_assets/app/terminal.js"),
    include_str!("../webapp_assets/app/events.js"),
);
const FAVICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <rect width="64" height="64" rx="14" fill="#101820"/>
  <path d="M16 18h18c8.8 0 16 7.2 16 16 0 3.2-.9 6.2-2.6 8.7L54 49.3
           49.3 54l-6.7-6.6A16 16 0 0 1 34 50H16V18Z" fill="#2dd4bf"/>
  <path d="M23 25h11a9 9 0 1 1 0 18H23V25Z" fill="#101820"/>
  <path d="M29 31h5a3 3 0 1 1 0 6h-5v-6Z" fill="#f8fafc"/>
  <circle cx="50" cy="16" r="5" fill="#facc15"/>
</svg>"##;
