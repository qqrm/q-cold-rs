mod adapter;
mod agents;
mod history;
mod repo_bundle;
mod repository;
mod state;
mod status;
mod telegram;
mod webapp;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{cmp::Reverse, collections::HashSet};

use agents::AgentArgs;
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::Value;
use telegram::TelegramArgs;

use crate::adapter::{BundleAdapter, ProofAdapter, TaskAdapter};
use crate::repository::{AdapterContext, RepositoryArgs, RepositoryConfig};

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
    let cli = Cli::parse_from(cargo_subcommand_args(std::env::args_os()));
    match cli.command {
        TopLevel::Build(args) => adapter_for_cwd_sensitive_repo()?.build(&args.args),
        TopLevel::Install(args) => adapter_for_cwd_sensitive_repo()?.install(&args.args),
        TopLevel::Task(cmd) => task_command(cmd),
        TopLevel::TaskRecord(args) => task_record_command(args),
        TopLevel::Bundle => repo_bundle::run(),
        TopLevel::Status => status::run(),
        TopLevel::Repo(args) => repository::run(args),
        TopLevel::Agent(args) => agents::run(args),
        TopLevel::Telegram(args) => telegram::run(args),
        TopLevel::Ci(args) => adapter_for_cwd_sensitive_repo()?.ci(&args.args),
        TopLevel::Verify(args) => adapter_for_cwd_sensitive_repo()?.verify(&args.args),
        TopLevel::Compat(args) => adapter_for_cwd_sensitive_repo()?.compat(&args.args),
        TopLevel::Ffi(args) => adapter_for_cwd_sensitive_repo()?.ffi(&args.args),
    }
}

fn cargo_subcommand_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args: Vec<OsString> = args.into_iter().collect();
    if let Some(program) = args.first_mut() {
        *program = OsString::from("qcold");
    }
    if args
        .get(1)
        .and_then(|arg| arg.to_str())
        .is_some_and(|arg| arg == "qcold")
    {
        args.remove(1);
    }
    args
}

#[derive(Parser)]
#[command(
    name = "qcold",
    about = "Q-COLD orchestration facade over adapter-backed task flow",
    after_help = "Examples:\n  qcold repo list\n  qcold repo add target-repo /path/to/target-repo --xtask-manifest /path/to/target-repo/xtask/Cargo.toml --set-active\n  qcold status\n  qcold task-record create --description \"Add task CRUD and automatic capture\"\n  qcold task-record list\n  qcold agent list\n  qcold agent start --track audit -- codex exec \"inspect repo\"\n  qcold telegram poll\n  qcold bundle\n  qcold task inspect runtime-audit\n  qcold task open my-task\n  qcold task enter\n  qcold task iteration-notify --message \"waiting for direction\"\n  qcold task closeout --outcome success --message \"docs: update truth\"\n  qcold verify fast\n  qcold ci matrix rust-all-on --jobs 4\n\nCargo subcommand compatibility is also supported: cargo qcold <command>."
)]
struct Cli {
    #[command(subcommand)]
    command: TopLevel,
}

#[derive(Subcommand)]
enum TopLevel {
    #[command(about = "Run the adapter-backed build entrypoint")]
    Build(PassthroughArgs),
    #[command(about = "Run the adapter-backed install entrypoint")]
    Install(PassthroughArgs),
    #[command(about = "Orchestrated task-flow commands")]
    Task(TaskArgs),
    #[command(about = "Manage Q-COLD-owned task records in SQLite")]
    TaskRecord(TaskRecordArgs),
    #[command(about = "Write one source ZIP archive for the current repository into ./bundles")]
    Bundle,
    #[command(about = "Summarize primary-checkout, worktree, and drift state")]
    Status,
    #[command(about = "Manage repository connections served by Q-COLD")]
    Repo(RepositoryArgs),
    #[command(about = "Start and inspect Q-COLD managed agent processes")]
    Agent(AgentArgs),
    #[command(about = "Run Telegram command/reply control-plane adapters")]
    Telegram(TelegramArgs),
    Ci(PassthroughArgs),
    Verify(PassthroughArgs),
    Compat(PassthroughArgs),
    Ffi(PassthroughArgs),
}

#[derive(Args)]
#[command(disable_help_flag = true)]
struct PassthroughArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<OsString>,
}

#[derive(Args)]
struct TaskArgs {
    #[command(subcommand)]
    command: TaskSubcommand,
}

#[derive(Args)]
struct TaskRecordArgs {
    #[command(subcommand)]
    command: TaskRecordSubcommand,
}

#[derive(Subcommand)]
enum TaskRecordSubcommand {
    #[command(about = "List Q-COLD-owned task records")]
    List {
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    #[command(about = "Show one Q-COLD-owned task record")]
    Show { id: String },
    #[command(about = "Create or update a Q-COLD-owned task record")]
    Create(TaskRecordCreateArgs),
    #[command(about = "Update title, description, or status for a task record")]
    Update(TaskRecordUpdateArgs),
    #[command(about = "Mark a task record closed")]
    Close {
        id: String,
        #[arg(long, default_value = "success")]
        outcome: String,
    },
    #[command(about = "Delete a task record")]
    Delete { id: String },
}

#[derive(Args)]
struct TaskRecordCreateArgs {
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    description: String,
    #[arg(long, default_value = "manual")]
    source: String,
    #[arg(long, default_value = "open")]
    status: String,
    #[arg(long)]
    repo_root: Option<PathBuf>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long)]
    agent_id: Option<String>,
}

#[derive(Args)]
struct TaskRecordUpdateArgs {
    id: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    status: Option<String>,
}

#[derive(Subcommand)]
enum TaskSubcommand {
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
    IterationNotify(MessageArgs),
    Closeout(CloseoutArgs),
    Finalize(MessageArgs),
    #[command(hide = true)]
    Bundle {
        #[arg(value_name = "task-id")]
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
        #[arg(long, default_value_t = 1)]
        max_age_hours: u64,
    },
}

#[derive(Args)]
struct MessageArgs {
    #[arg(long)]
    message: String,
}

#[derive(Args)]
struct CloseoutArgs {
    #[arg(long)]
    outcome: CloseoutOutcome,
    #[arg(long)]
    message: Option<String>,
    #[arg(long)]
    reason: Option<String>,
}

#[derive(Clone, ValueEnum)]
enum CloseoutOutcome {
    Success,
    Blocked,
    Failed,
}

impl CloseoutOutcome {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }
}

fn task_command(args: TaskArgs) -> Result<u8> {
    match args.command {
        TaskSubcommand::Inspect { topic } => {
            adapter_for_active_repo()?.inspect(topic.as_deref())
        }
        TaskSubcommand::Open { task_slug, profile } => {
            record_task_open(&task_slug, profile.as_deref())?;
            adapter_for_active_repo()?.open(&task_slug, profile.as_deref())
        }
        TaskSubcommand::Enter => adapter_for_cwd_sensitive_repo()?.enter(),
        TaskSubcommand::List => adapter_for_active_repo()?.list(),
        TaskSubcommand::TerminalCheck => adapter_for_active_repo()?.terminal_check(),
        TaskSubcommand::IterationNotify(args) => {
            adapter_for_cwd_sensitive_repo()?.iteration_notify(&args.message)
        }
        TaskSubcommand::Closeout(args) => {
            let cwd = std::env::current_dir().ok();
            let task_record_id = cwd
                .as_deref()
                .and_then(task_record_id_from_worktree);
            if let Some(worktree) = cwd.as_deref() {
                if let Err(err) = sync_task_flow_record_for_worktree(worktree, None) {
                    eprintln!("warning: failed to refresh Codex task token telemetry: {err:#}");
                }
            }
            let code = adapter_for_cwd_sensitive_repo()?.closeout(
                args.outcome.as_str(),
                args.message.as_deref(),
                args.reason.as_deref(),
            )?;
            if terminal_closeout_code(args.outcome.as_str(), code) {
                if let Some(id) = task_record_id {
                    state::update_task_record(
                        &id,
                        None,
                        None,
                        Some(&format!("closed:{}", args.outcome.as_str())),
                    )?;
                }
            }
            Ok(code)
        }
        TaskSubcommand::Finalize(args) => {
            adapter_for_cwd_sensitive_repo()?.finalize(&args.message)
        }
        TaskSubcommand::Bundle { task_id } => {
            adapter_for_cwd_sensitive_repo()?.task_bundle(task_id.as_deref())
        }
        TaskSubcommand::Clean { task_slug } => adapter_for_active_repo()?.clean(&task_slug),
        TaskSubcommand::Clear { task_slug } => adapter_for_active_repo()?.clear(&task_slug),
        TaskSubcommand::ClearAll => adapter_for_active_repo()?.clear_all(),
        TaskSubcommand::OrphanList => adapter_for_active_repo()?.orphan_list(),
        TaskSubcommand::OrphanClearStale { max_age_hours } => {
            adapter_for_active_repo()?.orphan_clear_stale(max_age_hours)
        }
    }
}

fn task_record_command(args: TaskRecordArgs) -> Result<u8> {
    match args.command {
        TaskRecordSubcommand::List { status, limit } => {
            sync_codex_task_records()?;
            let records = state::load_task_records(status.as_deref(), limit)?;
            if records.is_empty() {
                println!("task-records\tcount=0");
            } else {
                println!("task-records\tcount={}", records.len());
                for record in records {
                    println!("{}", render_task_record(&record));
                }
            }
        }
        TaskRecordSubcommand::Show { id } => {
            sync_codex_task_records()?;
            let record = state::get_task_record(&id)?
                .ok_or_else(|| anyhow::anyhow!("unknown task record: {id}"))?;
            println!("{}", render_task_record(&record));
            if !record.description.is_empty() {
                println!("description\t{}", record.description);
            }
        }
        TaskRecordSubcommand::Create(args) => {
            let record = task_record_from_create_args(args)?;
            state::upsert_task_record(&record)?;
            println!("{}", render_task_record(&record));
        }
        TaskRecordSubcommand::Update(args) => {
            let title = args.title.as_deref().map(polish_task_text);
            let description = args.description.as_deref().map(polish_task_text);
            state::update_task_record(
                &args.id,
                title.as_deref(),
                description.as_deref(),
                args.status.as_deref(),
            )?;
            let record = state::get_task_record(&args.id)?
                .ok_or_else(|| anyhow::anyhow!("unknown task record: {}", args.id))?;
            println!("{}", render_task_record(&record));
        }
        TaskRecordSubcommand::Close { id, outcome } => {
            let status = format!("closed:{outcome}");
            state::update_task_record(&id, None, None, Some(&status))?;
            let record = state::get_task_record(&id)?
                .ok_or_else(|| anyhow::anyhow!("unknown task record: {id}"))?;
            println!("{}", render_task_record(&record));
        }
        TaskRecordSubcommand::Delete { id } => {
            state::delete_task_record(&id)?;
            println!("task-record-deleted\t{id}");
        }
    }
    Ok(0)
}

fn task_record_from_create_args(args: TaskRecordCreateArgs) -> Result<state::TaskRecordRow> {
    let description = polish_task_text(&args.description);
    let title = args
        .title
        .as_deref()
        .map(polish_task_text)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| title_from_description(&description));
    let id = args
        .id
        .unwrap_or_else(|| format!("adhoc/{}-{}", unix_now(), slug_from_title(&title)));
    Ok(state::new_task_record(
        id,
        args.source,
        title,
        description,
        args.status,
        args.repo_root.map(|path| path.display().to_string()),
        args.cwd.map(|path| path.display().to_string()),
        args.agent_id,
        None,
    ))
}

fn record_task_open(task_slug: &str, profile: Option<&str>) -> Result<()> {
    let repo = repository::for_adapter_context(AdapterContext::ActiveRepository)?;
    let title = title_from_slug(task_slug);
    let description = format!("Open managed task-flow work for {title}.");
    let metadata = serde_json::json!({
        "task_slug": task_slug,
        "profile": profile,
        "kind": "managed-task-flow"
    });
    let record = state::new_task_record(
        format!("task/{task_slug}"),
        "task-flow".to_string(),
        title,
        description,
        "open".to_string(),
        Some(repo.root.display().to_string()),
        std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string()),
        None,
        Some(metadata.to_string()),
    );
    state::upsert_task_record(&record)
}

pub(crate) fn record_agent_task(record: &agents::AgentRecord) -> Result<()> {
    let command = agent_command_payload(&record.command);
    let Some(prompt) = prompt_from_agent_command(&command) else {
        return Ok(());
    };
    let description = polish_task_text(&prompt);
    if description.is_empty() {
        return Ok(());
    }
    let title = title_from_description(&description);
    let metadata = serde_json::json!({
        "kind": "agent-ad-hoc",
        "track": record.track,
        "command": command,
    });
    let record = state::new_task_record(
        format!("adhoc/{}-{}", record.started_at, slug_from_title(&title)),
        "agent".to_string(),
        title,
        description,
        "open".to_string(),
        repository::active_root()
            .ok()
            .map(|path| path.display().to_string()),
        std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string()),
        Some(record.id.clone()),
        Some(metadata.to_string()),
    );
    state::upsert_task_record(&record)
}

pub(crate) fn sync_codex_task_records() -> Result<usize> {
    let agent_rows = state::load_agents(&agents::registry_path()?)?;
    let existing = state::load_task_records(None, 1000)?;
    let preferred_cwd = repository::active_root()
        .ok()
        .or_else(|| std::env::current_dir().ok());
    let mut synced = 0;

    for agent in agent_rows {
        let command = agent_command_payload(&agent.command);
        let Some(account) = codex_account_from_agent_command(&command) else {
            continue;
        };
        let existing_record = existing
            .iter()
            .find(|record| record.agent_id.as_deref() == Some(agent.id.as_str()));
        let mut claimed_session_paths = claimed_codex_session_paths(&existing);
        if let Some(path) = existing_record.and_then(codex_session_path_from_task_record) {
            claimed_session_paths.remove(&path);
        }
        let summary =
            if let Some(path) = existing_record.and_then(codex_session_path_from_task_record) {
                parse_codex_session_summary(Path::new(&path))?
            } else {
                find_codex_session_summary(
                    &account,
                    agent.started_at,
                    &claimed_session_paths,
                    preferred_cwd.as_deref(),
                )?
            };
        let Some(summary) = summary else {
            continue;
        };
        let description = polish_task_text(&summary.prompt);
        if description.is_empty() {
            continue;
        }

        let title = existing_record
            .map(|record| record.title.clone())
            .unwrap_or_else(|| title_from_description(&description));
        let record_id = existing_record
            .map(|record| record.id.clone())
            .unwrap_or_else(|| format!("adhoc/{}-{}", agent.started_at, slug_from_title(&title)));
        let status = existing_record
            .map(|record| record.status.clone())
            .unwrap_or_else(|| {
                if summary.task_complete || !process_running(agent.pid) {
                    "closed:unknown".to_string()
                } else {
                    "open".to_string()
                }
            });
        let source = existing_record
            .map(|record| record.source.clone())
            .unwrap_or_else(|| "codex-session".to_string());
        let record_description = existing_record
            .map(|record| record.description.clone())
            .unwrap_or(description);
        let metadata = serde_json::json!({
            "kind": "codex-session-import",
            "agent_id": agent.id.clone(),
            "track": agent.track.clone(),
            "command": command,
            "codex_account": account,
            "session_path": summary.path.display().to_string(),
            "codex_thread_id": summary.thread_id,
            "codex_started_at": summary.started_at,
            "codex_cwd": summary.cwd,
            "token_usage": summary.token_usage,
            "rate_limits": summary.rate_limits,
            "task_complete": summary.task_complete,
        });
        let metadata_json = metadata.to_string();
        if let Some(existing_record) = existing_record {
            if existing_record.source == source
                && existing_record.title == title
                && existing_record.description == record_description
                && existing_record.status == status
                && existing_record.metadata_json.as_deref() == Some(metadata_json.as_str())
            {
                continue;
            }
        }
        let record = state::new_task_record(
            record_id,
            source,
            title,
            record_description,
            status,
            repository::active_root()
                .ok()
                .map(|path| path.display().to_string()),
            std::env::current_dir()
                .ok()
                .map(|path| path.display().to_string()),
            Some(agent.id),
            Some(metadata_json),
        );
        state::upsert_task_record(&record)?;
        synced += 1;
    }

    synced += sync_task_flow_records(&existing)?;

    Ok(synced)
}

#[derive(Debug)]
struct CodexSessionSummary {
    path: PathBuf,
    thread_id: Option<String>,
    started_at: Option<u64>,
    cwd: Option<String>,
    prompt: String,
    token_usage: Option<Value>,
    rate_limits: Option<Value>,
    task_complete: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CodexTokenUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
    model_calls: u64,
    model_context_window: Option<u64>,
}

impl CodexTokenUsage {
    fn add_last_usage(&mut self, info: &Value) -> bool {
        let Some(last) = info.get("last_token_usage") else {
            return false;
        };
        let input = last
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cached = last
            .get("cached_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = last
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let reasoning = last
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total = last
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(input + output);
        self.input_tokens += input;
        self.cached_input_tokens += cached;
        self.output_tokens += output;
        self.reasoning_output_tokens += reasoning;
        self.total_tokens += total;
        self.model_calls += 1;
        if self.model_context_window.is_none() {
            self.model_context_window = info.get("model_context_window").and_then(Value::as_u64);
        }
        true
    }

    fn as_json(self) -> Value {
        let non_cached_input = self.input_tokens.saturating_sub(self.cached_input_tokens);
        serde_json::json!({
            "input_tokens": self.input_tokens,
            "cached_input_tokens": self.cached_input_tokens,
            "non_cached_input_tokens": non_cached_input,
            "output_tokens": self.output_tokens,
            "reasoning_output_tokens": self.reasoning_output_tokens,
            "total_tokens": self.total_tokens,
            "displayed_total_tokens": non_cached_input + self.output_tokens,
            "model_calls": self.model_calls,
            "model_context_window": self.model_context_window,
            "source": "codex-session-window",
        })
    }
}

fn claimed_codex_session_paths(records: &[state::TaskRecordRow]) -> HashSet<String> {
    records
        .iter()
        .filter_map(codex_session_path_from_task_record)
        .collect()
}

fn codex_session_path_from_task_record(record: &state::TaskRecordRow) -> Option<String> {
    let metadata = serde_json::from_str::<Value>(record.metadata_json.as_deref()?).ok()?;
    metadata
        .get("session_path")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn sync_task_flow_records(records: &[state::TaskRecordRow]) -> Result<usize> {
    let mut synced = 0;
    for record in records {
        if record.source != "task-flow" {
            continue;
        }
        let Some(worktree) = task_flow_worktree_for_record(record) else {
            continue;
        };
        if sync_task_flow_record_for_worktree(&worktree, Some(record.status.as_str()))? {
            synced += 1;
        }
    }
    Ok(synced)
}

fn sync_task_flow_record_for_worktree(
    worktree: &Path,
    status_override: Option<&str>,
) -> Result<bool> {
    let env = parse_task_env(&worktree.join(".task/task.env"))?;
    let Some(record_id) = env.get("TASK_ID").cloned() else {
        return Ok(false);
    };
    let mut record = state::get_task_record(&record_id)?.unwrap_or_else(|| {
        let task_name = env
            .get("TASK_NAME")
            .cloned()
            .unwrap_or_else(|| record_id.trim_start_matches("task/").to_string());
        state::new_task_record(
            record_id.clone(),
            "task-flow".to_string(),
            title_from_slug(&task_name),
            env.get("TASK_DESCRIPTION")
                .cloned()
                .unwrap_or_else(|| format!("Managed task-flow work for {task_name}.")),
            "open".to_string(),
            env.get("PRIMARY_REPO_PATH").cloned(),
            Some(worktree.display().to_string()),
            None,
            None,
        )
    });
    let start = env
        .get("STARTED_AT")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(record.created_at);
    let task_is_open = env.get("STATUS").is_none_or(|status| status == "open");
    let finish = if task_is_open {
        unix_now()
    } else {
        env.get("UPDATED_AT")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_else(unix_now)
    };
    let usage = codex_token_usage_for_worktree(worktree, start, finish)?;

    let mut metadata = record
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    metadata.insert(
        "kind".to_string(),
        Value::String("managed-task-flow".to_string()),
    );
    if let Some(task_slug) = record_id.strip_prefix("task/") {
        metadata.insert("task_slug".to_string(), Value::String(task_slug.to_string()));
    }
    metadata.insert(
        "task_worktree".to_string(),
        Value::String(worktree.display().to_string()),
    );
    metadata.insert("task_started_at".to_string(), Value::from(start));
    if task_is_open {
        metadata.remove("task_finished_at");
    } else {
        metadata.insert("task_finished_at".to_string(), Value::from(finish));
    }
    if let Some(usage) = usage {
        metadata.insert("token_usage".to_string(), usage.as_json());
    }

    record.cwd = Some(worktree.display().to_string());
    if let Some(primary) = env.get("PRIMARY_REPO_PATH") {
        record.repo_root = Some(primary.clone());
    }
    if let Some(status) = status_override {
        record.status = status.to_string();
    } else if let Some(status) = env.get("STATUS") {
        record.status = status.clone();
    }
    let metadata_json = Value::Object(metadata).to_string();
    if record.metadata_json.as_deref() == Some(metadata_json.as_str()) {
        return Ok(false);
    }
    record.metadata_json = Some(metadata_json);
    record.updated_at = unix_now();
    state::upsert_task_record(&record)?;
    Ok(true)
}

fn task_flow_worktree_for_record(record: &state::TaskRecordRow) -> Option<PathBuf> {
    let metadata = record
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    if let Some(worktree) = metadata
        .as_ref()
        .and_then(|value| value.get("task_worktree"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.join(".task/task.env").is_file())
    {
        return Some(worktree);
    }
    let task_slug = metadata
        .as_ref()
        .and_then(|value| value.get("task_slug"))
        .and_then(Value::as_str)
        .or_else(|| record.id.strip_prefix("task/"))?;
    let repo_root = record.repo_root.as_deref().map(Path::new)?;
    let managed_root = managed_root_for(repo_root);
    let entries = fs::read_dir(managed_root).ok()?;
    for entry in entries.flatten() {
        let worktree = entry.path();
        let Ok(env) = parse_task_env(&worktree.join(".task/task.env")) else {
            continue;
        };
        if env
            .get("TASK_NAME")
            .is_some_and(|name| name == task_slug)
            || env
                .get("TASK_ID")
                .is_some_and(|id| id == &format!("task/{task_slug}"))
        {
            return Some(worktree);
        }
    }
    None
}

fn task_record_id_from_worktree(worktree: &Path) -> Option<String> {
    parse_task_env(&worktree.join(".task/task.env"))
        .ok()?
        .get("TASK_ID")
        .cloned()
}

fn parse_task_env(path: &Path) -> Result<std::collections::BTreeMap<String, String>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut entries = std::collections::BTreeMap::new();
    for line in content.lines() {
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let value = if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
            raw[1..raw.len() - 1].replace("'\\''", "'")
        } else {
            raw.to_string()
        };
        entries.insert(key.to_string(), value);
    }
    Ok(entries)
}

fn managed_root_for(primary_root: &Path) -> PathBuf {
    primary_root.parent().map_or_else(
        || primary_root.join("WT"),
        |parent| {
            parent
                .join("WT")
                .join(primary_root.file_name().unwrap_or_default())
        },
    )
}

fn terminal_closeout_code(outcome: &str, code: u8) -> bool {
    matches!(
        (outcome, code),
        ("success", 0) | ("blocked", 10) | ("failed", 11)
    )
}

fn codex_account_from_agent_command(command: &str) -> Option<String> {
    let lower = command.to_lowercase();
    if !(lower.contains("c2")
        || lower.contains("cc2")
        || lower.contains("codex")
        || lower.contains("code"))
    {
        return None;
    }

    for word in shell_words(command) {
        let Some(name) = Path::new(&word).file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name == "c2" || name == "cc2" {
            return Some("2".to_string());
        }
        if let Some(suffix) = name.strip_prefix("codex") {
            if !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
                return Some(suffix.to_string());
            }
        }
        if let Some(suffix) = name.strip_prefix('c') {
            if !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
                return Some(suffix.to_string());
            }
        }
    }

    if lower.contains("codex") {
        Some("2".to_string())
    } else {
        None
    }
}

fn find_codex_session_summary(
    account: &str,
    agent_started_at: u64,
    claimed_session_paths: &HashSet<String>,
    preferred_cwd: Option<&Path>,
) -> Result<Option<CodexSessionSummary>> {
    let root = codex_sessions_root(account)?;
    find_codex_session_summary_in_root(
        &root,
        agent_started_at,
        claimed_session_paths,
        preferred_cwd,
    )
}

fn codex_token_usage_for_worktree(
    worktree: &Path,
    started_at: u64,
    finished_at: u64,
) -> Result<Option<CodexTokenUsage>> {
    codex_token_usage_for_worktree_in_roots(worktree, started_at, finished_at, &codex_session_roots()?)
}

fn codex_token_usage_for_worktree_in_roots(
    worktree: &Path,
    started_at: u64,
    finished_at: u64,
    roots: &[PathBuf],
) -> Result<Option<CodexTokenUsage>> {
    let mut files = Vec::new();
    for root in roots {
        if root.is_dir() {
            collect_session_files(root, &mut files)?;
        }
    }
    let worktree_text = worktree.display().to_string();
    let mut usage = CodexTokenUsage::default();
    for path in files {
        let Some(modified) = modified_unix(&path) else {
            continue;
        };
        if modified < started_at.saturating_sub(300) || modified > finished_at.saturating_add(900)
        {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        if !content.contains(&worktree_text) {
            continue;
        }
        for line in content.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(timestamp) = value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339_unix)
            else {
                continue;
            };
            if timestamp < started_at || timestamp > finished_at {
                continue;
            }
            let Some(payload) = value.get("payload") else {
                continue;
            };
            if value.get("type").and_then(Value::as_str) == Some("event_msg")
                && payload.get("type").and_then(Value::as_str) == Some("token_count")
            {
                if let Some(info) = payload.get("info") {
                    usage.add_last_usage(info);
                }
            }
        }
    }
    Ok((usage.model_calls > 0).then_some(usage))
}

fn codex_session_roots() -> Result<Vec<PathBuf>> {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        return Ok(vec![PathBuf::from(home).join("sessions")]);
    }
    let home = std::env::var("HOME").context("HOME is required to locate Codex session telemetry")?;
    let accounts = PathBuf::from(home).join(".codex-accounts");
    let mut roots = Vec::new();
    if accounts.is_dir() {
        for entry in fs::read_dir(accounts)? {
            let path = entry?.path().join("sessions");
            if path.is_dir() {
                roots.push(path);
            }
        }
    }
    Ok(roots)
}

fn find_codex_session_summary_in_root(
    root: &Path,
    agent_started_at: u64,
    claimed_session_paths: &HashSet<String>,
    preferred_cwd: Option<&Path>,
) -> Result<Option<CodexSessionSummary>> {
    if !root.exists() {
        return Ok(None);
    }

    let mut files = Vec::new();
    collect_session_files(&root, &mut files)?;
    files.sort_by_key(|path| std::cmp::Reverse(modified_unix(path).unwrap_or(0)));
    files.retain(|path| {
        let path_display = path.display().to_string();
        if claimed_session_paths.contains(&path_display) {
            return false;
        }
        modified_unix(path)
            .map(|modified| modified >= agent_started_at.saturating_sub(300))
            .unwrap_or(false)
    });
    files.truncate(100);

    let mut candidates = Vec::new();
    for path in files {
        if let Some(summary) = parse_codex_session_summary(&path)? {
            if !codex_session_start_matches_agent(summary.started_at, agent_started_at) {
                continue;
            }
            let cwd_mismatch = !codex_session_cwd_matches(summary.cwd.as_deref(), preferred_cwd);
            let start_distance = summary
                .started_at
                .map(|started_at| started_at.abs_diff(agent_started_at))
                .unwrap_or(u64::MAX);
            let modified = modified_unix(&summary.path).unwrap_or(0);
            candidates.push((cwd_mismatch, start_distance, Reverse(modified), summary));
        }
    }
    candidates.sort_by_key(|(cwd_mismatch, start_distance, modified, _)| {
        (*cwd_mismatch, *start_distance, *modified)
    });
    Ok(candidates.into_iter().next().map(|(_, _, _, summary)| summary))
}

fn codex_sessions_root(account: &str) -> Result<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home)
            .join(".codex-accounts")
            .join(account)
            .join("sessions"));
    }
    anyhow::bail!("HOME is required to locate Codex session telemetry")
}

fn collect_session_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_session_files(&path, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    Ok(())
}

fn parse_codex_session_summary(path: &Path) -> Result<Option<CodexSessionSummary>> {
    let content = fs::read_to_string(path)?;
    let mut thread_id = None;
    let mut started_at = None;
    let mut cwd = None;
    let mut prompt = None;
    let mut fallback_prompt = None;
    let mut token_usage = None;
    let mut rate_limits = None;
    let mut task_complete = false;

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                if thread_id.is_none() {
                    thread_id = payload
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                if started_at.is_none() {
                    started_at = payload
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .and_then(parse_rfc3339_unix);
                }
                if cwd.is_none() {
                    cwd = payload
                        .get("cwd")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            Some("event_msg") => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                match payload.get("type").and_then(Value::as_str) {
                    Some("user_message") if prompt.is_none() => {
                        if let Some(message) = payload.get("message").and_then(Value::as_str) {
                            if is_meaningful_task_prompt(message) {
                                prompt = Some(message.trim().to_string());
                            }
                        }
                    }
                    Some("token_count") => {
                        if let Some(usage) = token_usage_summary(payload) {
                            token_usage = Some(usage);
                            rate_limits = payload.get("rate_limits").cloned();
                        }
                    }
                    Some("task_complete") => task_complete = true,
                    _ => {}
                }
            }
            Some("response_item") if fallback_prompt.is_none() => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                if payload.get("role").and_then(Value::as_str) == Some("user") {
                    if let Some(message) = response_item_text(payload) {
                        if is_meaningful_task_prompt(&message) {
                            fallback_prompt = Some(message);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let prompt = prompt.or(fallback_prompt);
    Ok(prompt.map(|prompt| CodexSessionSummary {
        path: path.to_path_buf(),
        thread_id: thread_id.or_else(|| codex_thread_id_from_path(path)),
        started_at,
        cwd,
        prompt,
        token_usage,
        rate_limits,
        task_complete,
    }))
}

fn codex_session_start_matches_agent(session_started_at: Option<u64>, agent_started_at: u64) -> bool {
    session_started_at
        .map(|started_at| {
            started_at >= agent_started_at.saturating_sub(300)
                && started_at <= agent_started_at.saturating_add(900)
        })
        .unwrap_or(true)
}

fn codex_session_cwd_matches(session_cwd: Option<&str>, preferred_cwd: Option<&Path>) -> bool {
    let Some(preferred_cwd) = preferred_cwd else {
        return true;
    };
    let Some(session_cwd) = session_cwd else {
        return true;
    };
    Path::new(session_cwd) == preferred_cwd
}

fn token_usage_summary(payload: &Value) -> Option<Value> {
    let info = payload.get("info")?;
    let total = info.get("total_token_usage")?;
    let input = total.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
    let cached = total
        .get("cached_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = total
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let non_cached_input = input.saturating_sub(cached);
    Some(serde_json::json!({
        "input_tokens": input,
        "cached_input_tokens": cached,
        "non_cached_input_tokens": non_cached_input,
        "output_tokens": output,
        "reasoning_output_tokens": total
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        "total_tokens": total
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(input + output),
        "displayed_total_tokens": non_cached_input + output,
        "last_token_usage": info.get("last_token_usage").cloned(),
        "model_context_window": info.get("model_context_window").cloned(),
    }))
}

fn response_item_text(payload: &Value) -> Option<String> {
    let content = payload.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("input_text"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .filter(|text| !text.contains("<environment_context>"))
        .collect::<Vec<_>>()
        .join(" ");
    if text.trim().is_empty() {
        None
    } else {
        Some(text.trim().to_string())
    }
}

fn is_meaningful_task_prompt(message: &str) -> bool {
    let text = message.trim();
    !text.is_empty()
        && text.chars().count() >= 5
        && !text.starts_with('/')
        && !text.starts_with("Token usage:")
        && !text.starts_with("To continue this session")
}

fn codex_thread_id_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
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

fn parse_rfc3339_unix(value: &str) -> Option<u64> {
    let (date, time) = value.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i64>().ok()?;
    let month = date_parts.next()?.parse::<i64>().ok()?;
    let day = date_parts.next()?.parse::<i64>().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let time = time.split_once('.').map_or(time, |(whole, _)| whole);
    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse::<u64>().ok()?;
    let minute = time_parts.next()?.parse::<u64>().ok()?;
    let second = time_parts.next()?.parse::<u64>().ok()?;
    if time_parts.next().is_some()
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    Some(days as u64 * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn modified_unix(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn process_running(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

fn render_task_record(record: &state::TaskRecordRow) -> String {
    format!(
        "task-record\t{}\tstatus={}\tsource={}\ttitle={}\trepo={}\tcwd={}\tagent={}\tupdated_at={}",
        record.id,
        record.status,
        record.source,
        record.title,
        record.repo_root.as_deref().unwrap_or(""),
        record.cwd.as_deref().unwrap_or(""),
        record.agent_id.as_deref().unwrap_or(""),
        record.updated_at
    )
}

fn agent_command_payload(command: &[String]) -> String {
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

fn prompt_from_agent_command(command: &str) -> Option<String> {
    let lower = command.to_lowercase();
    if !(lower.contains("c2") || lower.contains("cc2") || lower.contains("codex")) {
        return None;
    }
    let words = shell_words(command);
    let prompt = words
        .iter()
        .rev()
        .find(|word| {
            let clean = word.trim();
            clean.len() >= 3
                && !clean.starts_with('-')
                && clean != "/home/qqrm/.local/bin/c2"
                && clean != "c2"
                && clean != "cc2"
                && clean != "codex"
        })?
        .to_string();
    Some(prompt)
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

fn polish_task_text(value: &str) -> String {
    let mut text = value
        .replace(['\n', '\r', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for banned in [
        "блядь",
        "блять",
        "ёб",
        "еб",
        "ебан",
        "ёбан",
        "нахуй",
        "хуй",
        "пизд",
        "сука",
        "уебище",
        "уёбище",
        "fuck",
        "fucking",
        "shit",
    ] {
        text = replace_case_insensitive(&text, banned, "");
    }
    text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(", ,", ",")
        .replace(" ,", ",")
        .trim_matches(|ch: char| ch == ',' || ch == '.' || ch == ';' || ch == ':')
        .trim()
        .to_string();
    if text.is_empty() {
        "Task requested through Q-COLD.".to_string()
    } else {
        text
    }
}

fn replace_case_insensitive(input: &str, needle: &str, replacement: &str) -> String {
    let mut output = String::new();
    let mut rest = input;
    let needle_lower = needle.to_lowercase();
    while let Some(index) = rest.to_lowercase().find(&needle_lower) {
        output.push_str(&rest[..index]);
        output.push_str(replacement);
        rest = &rest[index + needle.len()..];
    }
    output.push_str(rest);
    output
}

fn title_from_slug(slug: &str) -> String {
    let title = slug
        .split(['-', '_', '/'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        "Q-COLD Task".to_string()
    } else {
        title
    }
}

fn title_from_description(description: &str) -> String {
    let words = description
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if words.is_empty() {
        "Q-COLD Task".to_string()
    } else {
        words
            .trim_matches(|ch: char| ch == ',' || ch == '.' || ch == ';' || ch == ':')
            .to_string()
    }
}

fn slug_from_title(title: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug.chars().take(64).collect()
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn adapter_for_active_repo() -> Result<adapter::XtaskProcessAdapter> {
    adapter_for_context(AdapterContext::ActiveRepository)
}

fn adapter_for_cwd_sensitive_repo() -> Result<adapter::XtaskProcessAdapter> {
    adapter_for_context(AdapterContext::CwdManagedWorktree)
}

fn adapter_for_context(context: AdapterContext) -> Result<adapter::XtaskProcessAdapter> {
    let repo = repository::for_adapter_context(context)?;
    repository_adapter_for(&repo)
}

fn repository_adapter_for(repo: &RepositoryConfig) -> Result<adapter::XtaskProcessAdapter> {
    if repo.adapter != "xtask-process" {
        anyhow::bail!(
            "repository {} uses unsupported adapter {}; supported adapter: xtask-process",
            repo.id,
            repo.adapter
        );
    }
    adapter::xtask_process_for(&repo.root, repo.xtask_manifest.as_deref())
}

#[cfg(test)]
mod tests {
    use super::{
        codex_token_usage_for_worktree_in_roots,
        cargo_subcommand_args, codex_account_from_agent_command,
        find_codex_session_summary_in_root, parse_codex_session_summary, parse_rfc3339_unix,
        polish_task_text, prompt_from_agent_command, slug_from_title,
    };
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn cargo_plugin_invocation_strips_subcommand_name() {
        assert_eq!(
            cargo_subcommand_args(os_args(&["cargo-qcold", "qcold", "status"])),
            os_args(&["qcold", "status"])
        );
    }

    #[test]
    fn direct_invocation_is_preserved() {
        assert_eq!(
            cargo_subcommand_args(os_args(&["qcold", "status"])),
            os_args(&["qcold", "status"])
        );
    }

    #[test]
    fn task_text_is_polished_for_storage() {
        assert_eq!(
            polish_task_text("Сделай, блядь, CRUD для задач"),
            "Сделай, CRUD для задач"
        );
    }

    #[test]
    fn c2_command_prompt_is_detected() {
        assert_eq!(
            prompt_from_agent_command("/home/qqrm/.local/bin/c2 \"Добавь CRUD для задач\"")
                .as_deref(),
            Some("Добавь CRUD для задач")
        );
    }

    #[test]
    fn codex_account_is_detected_from_cc2_wrapper() {
        assert_eq!(
            codex_account_from_agent_command("/home/qqrm/.local/bin/cc2").as_deref(),
            Some("2")
        );
        assert_eq!(
            codex_account_from_agent_command("/usr/bin/codex3 exec inspect").as_deref(),
            Some("3")
        );
    }

    #[test]
    fn codex_session_summary_imports_prompt_and_token_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(
            "rollout-2026-05-04T09-27-19-019df1ab-7579-7e41-ad71-701b63175455.jsonl",
        );
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"019df1ab-7579-7e41-ad71-701b63175455\",\"timestamp\":\"2026-05-04T09:27:19Z\",\"cwd\":\"/workspace/repo\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Сделай CRUD для задач\",\"images\":[]}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":40,\"output_tokens\":9,\"reasoning_output_tokens\":3,\"total_tokens\":109},\"last_token_usage\":{\"input_tokens\":100},\"model_context_window\":258400},\"rate_limits\":{\"plan_type\":\"pro\"}}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n"
            ),
        )
        .unwrap();

        let summary = parse_codex_session_summary(&path).unwrap().unwrap();
        assert_eq!(summary.prompt, "Сделай CRUD для задач");
        assert_eq!(
            summary.thread_id.as_deref(),
            Some("019df1ab-7579-7e41-ad71-701b63175455")
        );
        assert!(summary.task_complete);
        assert_eq!(
            summary.started_at,
            Some(parse_rfc3339_unix("2026-05-04T09:27:19Z").unwrap())
        );
        assert_eq!(summary.cwd.as_deref(), Some("/workspace/repo"));
        let usage = summary.token_usage.unwrap();
        assert_eq!(usage["non_cached_input_tokens"], 60);
        assert_eq!(usage["displayed_total_tokens"], 69);
    }

    #[test]
    fn codex_token_usage_for_worktree_sums_last_usage_window() {
        let temp = tempfile::tempdir().unwrap();
        let session_dir = temp.path().join("sessions/2026/05/06");
        let worktree = temp.path().join("WT/qcold/anchor-token-task");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join(
                "rollout-2026-05-06T00-00-00-019df1ab-7579-7e41-ad71-701b63175455.jsonl",
            ),
            format!(
                "{{\"timestamp\":\"1970-01-01T00:00:01.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"019df1ab-7579-7e41-ad71-701b63175455\",\"timestamp\":\"1970-01-01T00:00:01Z\",\"cwd\":\"{}\"}}}}\n\
                 {{\"timestamp\":\"1970-01-01T00:00:02.000Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"function_call\",\"arguments\":\"{{\\\"workdir\\\":\\\"{}\\\"}}\"}}}}\n\
                 {{\"timestamp\":\"1970-01-01T00:00:03.000Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"last_token_usage\":{{\"input_tokens\":10,\"cached_input_tokens\":4,\"output_tokens\":2,\"reasoning_output_tokens\":1,\"total_tokens\":12}},\"model_context_window\":258400}}}}}}\n\
                 {{\"timestamp\":\"1970-01-01T00:00:04.000Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"last_token_usage\":{{\"input_tokens\":7,\"cached_input_tokens\":5,\"output_tokens\":3,\"reasoning_output_tokens\":2,\"total_tokens\":10}},\"model_context_window\":258400}}}}}}\n",
                worktree.display(),
                worktree.display()
            ),
        )
        .unwrap();

        let usage =
            codex_token_usage_for_worktree_in_roots(&worktree, 0, u64::MAX, &[temp.path().join("sessions")])
                .unwrap()
                .unwrap();
        assert_eq!(usage.model_calls, 2);
        assert_eq!(usage.input_tokens, 17);
        assert_eq!(usage.cached_input_tokens, 9);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.reasoning_output_tokens, 3);
        assert_eq!(usage.total_tokens, 22);
    }

    #[test]
    fn codex_session_matcher_uses_session_start_and_claims() {
        let dir = tempfile::tempdir().unwrap();
        let claimed = dir.path().join(
            "rollout-1970-01-01T00-00-10-019df1ab-7579-7e41-ad71-701b63175455.jsonl",
        );
        let selected = dir.path().join(
            "rollout-1970-01-01T00-00-11-019df1ab-7579-7e41-ad71-701b63175456.jsonl",
        );
        let stale = dir.path().join(
            "rollout-1970-01-01T00-30-00-019df1ab-7579-7e41-ad71-701b63175457.jsonl",
        );
        write_session(&claimed, "1970-01-01T00:00:10Z", "/workspace/repo", "claimed");
        write_session(&selected, "1970-01-01T00:00:11Z", "/workspace/repo", "selected");
        write_session(&stale, "1970-01-01T00:30:00Z", "/workspace/repo", "stale");

        let claimed_paths = HashSet::from([claimed.display().to_string()]);
        let summary = find_codex_session_summary_in_root(
            dir.path(),
            10,
            &claimed_paths,
            Some(Path::new("/workspace/repo")),
        )
        .unwrap()
        .unwrap();
        assert_eq!(summary.path, selected);
        assert_eq!(summary.prompt, "selected");
    }

    #[test]
    fn rfc3339_timestamp_parser_handles_codex_session_meta() {
        assert_eq!(
            parse_rfc3339_unix("1970-01-02T00:00:01.123Z"),
            Some(86_401)
        );
    }

    #[test]
    fn task_slug_is_ascii_and_stable() {
        assert_eq!(slug_from_title("Fix task CRUD flow"), "fix-task-crud-flow");
        assert_eq!(slug_from_title("Задача"), "task");
    }

    fn write_session(path: &Path, timestamp: &str, cwd: &str, prompt: &str) {
        fs::write(
            path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"019df1ab-7579-7e41-ad71-701b63175455\",\"timestamp\":\"{timestamp}\",\"cwd\":\"{cwd}\"}}}}\n\
                 {{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"{prompt}\",\"images\":[]}}}}\n"
            ),
        )
        .unwrap();
    }
}
