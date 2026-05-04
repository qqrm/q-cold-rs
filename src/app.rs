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
use std::path::PathBuf;
use std::process::ExitCode;

use agents::AgentArgs;
use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
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
        TaskSubcommand::Closeout(args) => adapter_for_cwd_sensitive_repo()?.closeout(
            args.outcome.as_str(),
            args.message.as_deref(),
            args.reason.as_deref(),
        ),
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
        cargo_subcommand_args, polish_task_text, prompt_from_agent_command, slug_from_title,
    };
    use std::ffi::OsString;

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
    fn task_slug_is_ascii_and_stable() {
        assert_eq!(slug_from_title("Fix task CRUD flow"), "fix-task-crud-flow");
        assert_eq!(slug_from_title("Задача"), "task");
    }
}
