#[derive(Serialize)]
pub(crate) struct DashboardState {
    pub(crate) generated_at_unix: u64,
    pub(crate) app_build_id: String,
    pub(crate) daemon_cwd: String,
    pub(crate) repository: RepositoryContext,
    pub(crate) repositories: Vec<RepositoryContext>,
    pub(crate) node: crate::node_agent::NodeSnapshot,
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
    pub(crate) run: Option<WebQueueRun>,
    pub(crate) records: Vec<WebQueueItem>,
    pub(crate) tabs: Vec<WebQueueTab>,
    pub(crate) active_tab_id: String,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct WebQueueRun {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) execution_mode: String,
    pub(crate) execution_host: String,
    pub(crate) selected_agent_command: String,
    pub(crate) remote_launcher: Option<String>,
    pub(crate) remote_agent_local_proxy: Option<String>,
    pub(crate) remote_agent_remote_proxy: Option<String>,
    pub(crate) selected_repo_root: Option<String>,
    pub(crate) selected_repo_name: Option<String>,
    pub(crate) track: String,
    pub(crate) current_index: i64,
    pub(crate) stop_requested: bool,
    pub(crate) message: String,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

impl WebQueueRun {
    fn from_row(row: &state::QueueRunRow) -> Self {
        Self {
            id: row.id.clone(),
            status: row.status.as_str().to_string(),
            execution_mode: row.execution_mode.as_str().to_string(),
            execution_host: row.execution_host.as_str().to_string(),
            selected_agent_command: row.selected_agent_command.clone(),
            remote_launcher: row.remote_launcher.clone(),
            remote_agent_local_proxy: row.remote_agent_local_proxy.clone(),
            remote_agent_remote_proxy: row.remote_agent_remote_proxy.clone(),
            selected_repo_root: row.selected_repo_root.clone(),
            selected_repo_name: row.selected_repo_name.clone(),
            track: row.track.clone(),
            current_index: row.current_index,
            stop_requested: row.stop_requested,
            message: row.message.clone(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct WebQueueItem {
    pub(crate) id: String,
    pub(crate) run_id: String,
    pub(crate) position: i64,
    pub(crate) depends_on: Vec<String>,
    pub(crate) prompt: String,
    pub(crate) slug: String,
    pub(crate) repo_root: Option<String>,
    pub(crate) repo_name: Option<String>,
    pub(crate) execution_host: String,
    pub(crate) agent_command: String,
    pub(crate) remote_launcher: Option<String>,
    pub(crate) remote_agent_local_proxy: Option<String>,
    pub(crate) remote_agent_remote_proxy: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) status: String,
    pub(crate) message: String,
    pub(crate) attempts: i64,
    pub(crate) recovery_attempts: i64,
    pub(crate) next_attempt_at: Option<u64>,
    pub(crate) started_at: u64,
    pub(crate) updated_at: u64,
}

impl WebQueueItem {
    fn from_row(row: &state::QueueItemRow) -> Self {
        Self {
            id: row.id.clone(),
            run_id: row.run_id.clone(),
            position: row.position,
            depends_on: row.depends_on.clone(),
            prompt: row.prompt.clone(),
            slug: row.slug.clone(),
            repo_root: row.repo_root.clone(),
            repo_name: row.repo_name.clone(),
            execution_host: row.execution_host.as_str().to_string(),
            agent_command: row.agent_command.clone(),
            remote_launcher: row.remote_launcher.clone(),
            remote_agent_local_proxy: row.remote_agent_local_proxy.clone(),
            remote_agent_remote_proxy: row.remote_agent_remote_proxy.clone(),
            agent_id: row.agent_id.clone(),
            status: row.status.as_str().to_string(),
            message: row.message.clone(),
            attempts: row.attempts,
            recovery_attempts: row.recovery_attempts,
            next_attempt_at: row.next_attempt_at,
            started_at: row.started_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct WebQueueTab {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) run_id: Option<String>,
    pub(crate) run: Option<WebQueueRun>,
    pub(crate) records: Vec<WebQueueItem>,
    pub(crate) is_default: bool,
    pub(crate) active: bool,
    pub(crate) running: bool,
    pub(crate) status: String,
    pub(crate) count: usize,
    pub(crate) message: String,
    pub(crate) updated_at: u64,
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
    pub(crate) duration_seconds: Option<u64>,
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
        let duration_seconds = task_record_duration_seconds(&row, metadata.as_ref());
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
            duration_seconds,
            token_usage,
            token_efficiency,
        }
    }
}

fn task_record_duration_seconds(row: &state::TaskRecordRow, metadata: Option<&Value>) -> Option<u64> {
    metadata
        .and_then(|value| value.get("task_duration_seconds"))
        .and_then(Value::as_u64)
        .or_else(|| {
            let start = metadata
                .and_then(|value| value.get("task_started_at"))
                .and_then(Value::as_u64)?;
            let finish = metadata
                .and_then(|value| value.get("task_finished_at"))
                .and_then(Value::as_u64)?;
            (finish >= start).then_some(finish - start)
        })
        .or_else(|| {
            row.status
                .starts_with("closed:")
                .then_some(row.updated_at.saturating_sub(row.created_at))
        })
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
    transcript_path: Option<String>,
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
    pub(crate) tab_id: Option<String>,
    pub(crate) execution_mode: Option<String>,
    pub(crate) selected_execution_host: Option<String>,
    pub(crate) selected_agent_command: String,
    pub(crate) selected_remote_launcher: Option<String>,
    pub(crate) selected_remote_agent_local_proxy: Option<String>,
    pub(crate) selected_remote_agent_remote_proxy: Option<String>,
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
    pub(crate) wave_id: Option<String>,
    pub(crate) wave_index: Option<usize>,
    pub(crate) repo_root: Option<String>,
    pub(crate) repo_name: Option<String>,
    pub(crate) execution_host: Option<String>,
    pub(crate) agent_command: Option<String>,
    pub(crate) remote_launcher: Option<String>,
    pub(crate) remote_agent_local_proxy: Option<String>,
    pub(crate) remote_agent_remote_proxy: Option<String>,
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
    wave_id: Option<String>,
    wave_index: Option<usize>,
    repo_root: Option<String>,
    repo_name: Option<String>,
    execution_host: Option<String>,
    agent_command: Option<String>,
    remote_launcher: Option<String>,
    remote_agent_local_proxy: Option<String>,
    remote_agent_remote_proxy: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct QueueStopRequest {
    pub(crate) run_id: Option<String>,
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
pub(crate) struct QueueTabCreateRequest {
    pub(crate) label: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct QueueTabRequest {
    pub(crate) tab_id: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) queue_graph: Option<QueueGraphDiagnostics>,
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
const QUEUE_CSS: &str = include_str!("../webapp_assets/queue.css");
include!("../webapp_assets/app_js_assets.rs");

macro_rules! concat_app_js_assets {
    ($($asset:literal),+ $(,)?) => {
        concat!($(include_str!(concat!("../webapp_assets/app/", $asset)),)+)
    };
}

const APP_JS: &str = qcold_app_js_assets!(concat_app_js_assets);

#[cfg(test)]
macro_rules! app_js_asset_path_array {
    ($($asset:literal),+ $(,)?) => {
        &[$(concat!("src/webapp_assets/app/", $asset)),+]
    };
}

#[cfg(test)]
fn app_js_asset_paths() -> &'static [&'static str] {
    qcold_app_js_assets!(app_js_asset_path_array)
}
const FAVICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <rect width="64" height="64" rx="14" fill="#101820"/>
  <path d="M16 18h18c8.8 0 16 7.2 16 16 0 3.2-.9 6.2-2.6 8.7L54 49.3
           49.3 54l-6.7-6.6A16 16 0 0 1 34 50H16V18Z" fill="#2dd4bf"/>
  <path d="M23 25h11a9 9 0 1 1 0 18H23V25Z" fill="#101820"/>
  <path d="M29 31h5a3 3 0 1 1 0 6h-5v-6Z" fill="#f8fafc"/>
  <circle cx="50" cy="16" r="5" fill="#facc15"/>
</svg>"##;
