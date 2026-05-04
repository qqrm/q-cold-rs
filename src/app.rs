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
use std::process::ExitCode;

use agents::AgentArgs;
use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use telegram::TelegramArgs;

use crate::adapter::{BundleAdapter, ProofAdapter, TaskAdapter};
use crate::repository::{RepositoryArgs, RepositoryConfig};

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
        TopLevel::Build(args) => adapter_for_active_repo()?.build(&args.args),
        TopLevel::Install(args) => adapter_for_active_repo()?.install(&args.args),
        TopLevel::Task(cmd) => task_command(cmd),
        TopLevel::Bundle => repo_bundle::run(),
        TopLevel::Status => status::run(),
        TopLevel::Repo(args) => repository::run(args),
        TopLevel::Agent(args) => agents::run(args),
        TopLevel::Telegram(args) => telegram::run(args),
        TopLevel::Ci(args) => adapter_for_active_repo()?.ci(&args.args),
        TopLevel::Verify(args) => adapter_for_active_repo()?.verify(&args.args),
        TopLevel::Compat(args) => adapter_for_active_repo()?.compat(&args.args),
        TopLevel::Ffi(args) => adapter_for_active_repo()?.ffi(&args.args),
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
    after_help = "Examples:\n  qcold repo list\n  qcold repo add target-repo /path/to/target-repo --xtask-manifest /path/to/target-repo/xtask/Cargo.toml --set-active\n  qcold status\n  qcold agent list\n  qcold agent start --track audit -- codex exec \"inspect repo\"\n  qcold telegram poll\n  qcold bundle\n  qcold task inspect runtime-audit\n  qcold task open my-task\n  qcold task enter\n  qcold task iteration-notify --message \"waiting for direction\"\n  qcold task closeout --outcome success --message \"docs: update truth\"\n  qcold verify fast\n  qcold ci matrix rust-all-on --jobs 4\n\nCargo subcommand compatibility is also supported: cargo qcold <command>."
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
    let adapter = adapter_for_active_repo()?;
    match args.command {
        TaskSubcommand::Inspect { topic } => adapter.inspect(topic.as_deref()),
        TaskSubcommand::Open { task_slug, profile } => adapter.open(&task_slug, profile.as_deref()),
        TaskSubcommand::Enter => adapter.enter(),
        TaskSubcommand::List => adapter.list(),
        TaskSubcommand::TerminalCheck => adapter.terminal_check(),
        TaskSubcommand::IterationNotify(args) => adapter.iteration_notify(&args.message),
        TaskSubcommand::Closeout(args) => adapter.closeout(
            args.outcome.as_str(),
            args.message.as_deref(),
            args.reason.as_deref(),
        ),
        TaskSubcommand::Finalize(args) => adapter.finalize(&args.message),
        TaskSubcommand::Bundle { task_id } => adapter.task_bundle(task_id.as_deref()),
        TaskSubcommand::Clean { task_slug } => adapter.clean(&task_slug),
        TaskSubcommand::Clear { task_slug } => adapter.clear(&task_slug),
        TaskSubcommand::ClearAll => adapter.clear_all(),
        TaskSubcommand::OrphanList => adapter.orphan_list(),
        TaskSubcommand::OrphanClearStale { max_age_hours } => {
            adapter.orphan_clear_stale(max_age_hours)
        }
    }
}

fn adapter_for_active_repo() -> Result<adapter::XtaskProcessAdapter> {
    let repo = repository::active()?;
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
    use super::cargo_subcommand_args;
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
}
