mod adapter;
mod agents;
mod prompt;
mod repo_bundle;
mod repository;
mod state;
mod status;
mod telegram;
#[cfg(test)]
mod test_support;
mod webapp;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use agents::AgentArgs;
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::Value;
use telegram::TelegramArgs;

use crate::adapter::{BundleAdapter, ProofAdapter, TaskAdapter};
use crate::repository::{AdapterContext, RepositoryArgs, RepositoryConfig};

const QCOLD_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    ".",
    env!("QCOLD_BUILD_NUMBER"),
    " ",
    env!("QCOLD_BUILD_GIT_HASH")
);
const QCOLD_AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  qcold repo list\n",
    "  qcold repo add target-repo /path/to/target-repo ",
    "--xtask-manifest /path/to/target-repo/xtask/Cargo.toml --set-active\n",
    "  qcold status\n",
    "  qcold task-record create --description \"Add task CRUD and automatic capture\"\n",
    "  qcold task-record list\n",
    "  qcold agent list\n",
    "  qcold agent start --track audit -- codex exec \"inspect repo\"\n",
    "  qcold telegram poll\n",
    "  qcold bundle\n",
    "  qcold guard -- rg -n \"needle\" src\n",
    "  qcold task inspect runtime-audit\n",
    "  qcold task open my-task\n",
    "  qcold task enter\n",
    "  qcold task iteration-notify --message \"waiting for direction\"\n",
    "  qcold task pause --reason \"waiting for operator decision\"\n",
    "  qcold task closeout --outcome success --message \"docs: update truth\"\n",
    "  qcold verify fast\n",
    "  qcold ci matrix rust-all-on --jobs 4\n\n",
    "Cargo subcommand compatibility is also supported: cargo qcold <command>."
);
const DEFAULT_CODEX_TELEMETRY_RETENTION_HOURS: u64 = 48;
const LARGE_TOOL_OUTPUT_TOKEN_THRESHOLD: u64 = 5_000;
const MAX_TOOL_OUTPUT_SAMPLES: usize = 5;

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
        TopLevel::Guard(args) => guard_command(&args),
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
    version = QCOLD_VERSION,
    about = "Q-COLD orchestration facade over adapter-backed task flow",
    after_help = QCOLD_AFTER_HELP
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
    #[command(about = "Run a command and suppress oversized output")]
    Guard(GuardArgs),
}

#[derive(Args)]
#[command(disable_help_flag = true)]
struct PassthroughArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<OsString>,
}

#[derive(Args)]
struct GuardArgs {
    #[arg(long, default_value_t = 16_384)]
    max_bytes: usize,
    #[arg(long, default_value_t = 400)]
    max_lines: usize,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    command: Vec<OsString>,
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
    Pause(PauseArgs),
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
        #[arg(long, default_value_t = 2)]
        max_age_hours: u64,
    },
}

#[derive(Args)]
struct MessageArgs {
    #[arg(long)]
    message: String,
}

#[derive(Args)]
struct PauseArgs {
    #[arg(long)]
    reason: String,
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
            let record = record_task_open(&task_slug, profile.as_deref())?;
            adapter_for_active_repo()?.open(
                &task_slug,
                profile.as_deref(),
                record.sequence,
                task_prompt_from_record(&record).as_deref(),
            )
        }
        TaskSubcommand::Enter => adapter_for_cwd_sensitive_repo()?.enter(),
        TaskSubcommand::List => adapter_for_active_repo()?.list(),
        TaskSubcommand::TerminalCheck => adapter_for_active_repo()?.terminal_check(),
        TaskSubcommand::IterationNotify(args) => {
            adapter_for_cwd_sensitive_repo()?.iteration_notify(&args.message)
        }
        TaskSubcommand::Pause(args) => {
            let cwd = std::env::current_dir().ok();
            let task_record_id = cwd.as_deref().and_then(task_record_id_from_worktree);
            let code = adapter_for_cwd_sensitive_repo()?.pause(&args.reason)?;
            if code == 0 {
                if let Some(id) = task_record_id {
                    state::update_task_record(&id, None, None, Some("paused"))?;
                }
            }
            Ok(code)
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
            if let Some(line) = render_task_record_token_usage(&record) {
                println!("{line}");
            }
            if let Some(line) = render_task_record_token_efficiency(&record) {
                println!("{line}");
            }
        }
        TaskRecordSubcommand::Create(args) => {
            let record = task_record_from_create_args(args);
            let record = state::upsert_task_record(&record)?;
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

fn task_record_from_create_args(args: TaskRecordCreateArgs) -> state::TaskRecordRow {
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
    state::new_task_record(
        id,
        args.source,
        title,
        description,
        args.status,
        args.repo_root.map(|path| path.display().to_string()),
        args.cwd.map(|path| path.display().to_string()),
        args.agent_id,
        None,
    )
}

fn record_task_open(task_slug: &str, profile: Option<&str>) -> Result<state::TaskRecordRow> {
    let repo = repository::for_adapter_context(AdapterContext::ActiveRepository)?;
    let title = title_from_slug(task_slug);
    let original_prompt = env_prompt("QCOLD_TASKFLOW_PROMPT");
    let prompt_snippet = original_prompt.as_deref().map(prompt::prompt_snippet);
    let description = prompt_snippet
        .as_deref()
        .filter(|value| !value.is_empty())
        .map_or_else(
            || format!("Open managed task-flow work for {title}."),
            str::to_string,
        );
    let metadata = serde_json::json!({
        "task_slug": task_slug,
        "profile": profile,
        "kind": "managed-task-flow",
        "operator_prompt": original_prompt,
        "operator_prompt_snippet": prompt_snippet,
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

fn env_prompt(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn task_prompt_from_record(record: &state::TaskRecordRow) -> Option<String> {
    let metadata = serde_json::from_str::<Value>(record.metadata_json.as_deref()?).ok()?;
    metadata
        .get("operator_prompt")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

pub(crate) fn record_agent_task(record: &agents::AgentRecord) -> Result<()> {
    if is_queue_agent_track(&record.track) {
        return Ok(());
    }
    let command = agent_command_payload(&record.command);
    let Some(prompt) = prompt_from_agent_command(&command) else {
        return Ok(());
    };
    let description = polish_task_text(&prompt);
    if description.is_empty() {
        return Ok(());
    }
    let title = title_from_description(&description);
    let cwd = record.cwd.clone().or_else(|| std::env::current_dir().ok());
    let managed_task_record_id = cwd.as_deref().and_then(task_record_id_from_worktree);
    let repo_root = repo_root_for_agent_cwd(cwd.as_deref()).or_else(|| {
        repository::active_root()
            .ok()
            .map(|path| path.display().to_string())
    });
    let metadata = serde_json::json!({
        "kind": if managed_task_record_id.is_some() { "managed-agent-task-flow" } else { "agent-ad-hoc" },
        "track": record.track,
        "command": command,
    });
    let record = state::new_task_record(
        managed_task_record_id
            .clone()
            .unwrap_or_else(|| format!("adhoc/{}-{}", record.started_at, slug_from_title(&title))),
        if managed_task_record_id.is_some() {
            "task-flow".to_string()
        } else {
            "agent".to_string()
        },
        title,
        description,
        "open".to_string(),
        repo_root,
        cwd.map(|path| path.display().to_string()),
        Some(record.id.clone()),
        Some(metadata.to_string()),
    );
    state::upsert_task_record(&record).map(|_| ())
}


include!("app/codex_metadata.rs");
include!("app/task_flow_sync.rs");
include!("app/codex_sessions.rs");
include!("app/rendering.rs");
include!("app/tests.rs");

fn guard_command(args: &GuardArgs) -> Result<u8> {
    let Some((program, command_args)) = args.command.split_first() else {
        anyhow::bail!("guard requires a command");
    };
    let output = Command::new(program)
        .args(command_args)
        .output()
        .with_context(|| format!("failed to run guarded command {}", program.to_string_lossy()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let output_text = [stdout.trim_end(), stderr.trim_end()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let bytes = output_text.len();
    let lines = output_text.lines().count();
    if bytes > args.max_bytes || lines > args.max_lines {
        eprintln!(
            "qcold-guard\tstatus=blocked\tbytes={bytes}\tlines={lines}\tmax_bytes={}\
             \tmax_lines={}\tmessage=output too large; rerun with a narrower command or write raw \
             output to a task-local file and inspect a focused slice",
            args.max_bytes,
            args.max_lines,
        );
        return Ok(2);
    }
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    Ok(u8::try_from(output.status.code().unwrap_or(1)).unwrap_or(1))
}
