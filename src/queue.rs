use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{agents, repository, state, webapp};

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;

const DEFAULT_LISTEN: &str = "127.0.0.1:8787";
const MAX_PROMPT_PACKAGE_FILES: usize = 200;
const QUEUE_HELP: &str = include_str!("queue_help.txt");

#[derive(Args)]
#[command(after_help = QUEUE_HELP)]
pub(crate) struct QueueArgs {
    #[command(subcommand)]
    command: QueueCommand,
}

#[derive(Subcommand)]
enum QueueCommand {
    #[command(about = "Submit a new queue run to the dashboard daemon")]
    Run(QueueRunArgs),
    #[command(about = "Create an empty queue tab")]
    Create(QueueCreateArgs),
    #[command(about = "Switch the active queue tab")]
    Switch(QueueSwitchArgs),
    #[command(about = "Delete an inactive queue tab with no running work")]
    Delete(QueueDeleteArgs),
    #[command(about = "Append prompt items to an existing queue run")]
    Append(QueueAppendArgs),
    #[command(about = "List the current queue run from local Q-COLD state")]
    List,
    #[command(about = "Request a stop for the current queue run")]
    Stop(QueueClientArgs),
    #[command(about = "Continue a stopped queue run")]
    Continue(QueueContinueArgs),
    #[command(about = "Clear queued items and related task/agent artifacts")]
    Clear(QueueClearArgs),
}

#[derive(Args)]
struct QueueRunArgs {
    #[command(flatten)]
    source: QueuePromptSourceArgs,
    #[arg(long)]
    run_id: Option<String>,
    #[arg(long, help = "Queue tab id; defaults to the active queue tab")]
    tab_id: Option<String>,
    #[arg(long, value_enum, default_value_t = QueueExecutionMode::Sequence)]
    execution_mode: QueueExecutionMode,
    #[arg(long = "agent")]
    selected_agent_command: Option<String>,
    #[arg(long)]
    repo_root: Option<PathBuf>,
    #[arg(long)]
    repo_name: Option<String>,
    #[command(flatten)]
    client: QueueClientArgs,
}

#[derive(Args)]
struct QueueAppendArgs {
    #[arg(help = "Queue run id; defaults to the current stored queue run")]
    run_id: Option<String>,
    #[command(flatten)]
    source: QueuePromptSourceArgs,
    #[arg(long = "agent")]
    agent_command: Option<String>,
    #[arg(long)]
    repo_root: Option<PathBuf>,
    #[arg(long)]
    repo_name: Option<String>,
    #[command(flatten)]
    client: QueueClientArgs,
}

#[derive(Args)]
struct QueuePromptSourceArgs {
    #[arg(long, value_name = "PATH")]
    from: Option<PathBuf>,
    #[arg(long = "prompt")]
    prompts: Vec<String>,
    #[arg(long)]
    slug: Option<String>,
}

#[derive(Clone, Args)]
struct QueueClientArgs {
    #[arg(long, default_value = DEFAULT_LISTEN)]
    listen: String,
    #[arg(long, help = "Fail instead of auto-starting the dashboard daemon")]
    no_start_daemon: bool,
}

#[derive(Args)]
struct QueueContinueArgs {
    #[arg(help = "Queue run id; defaults to the current stored queue run")]
    run_id: Option<String>,
    #[command(flatten)]
    client: QueueClientArgs,
}

#[derive(Args)]
struct QueueClearArgs {
    #[arg(long, help = "Queue run id; defaults to the current stored queue run")]
    run_id: Option<String>,
    #[command(flatten)]
    client: QueueClientArgs,
}

#[derive(Clone, Copy, ValueEnum)]
enum QueueExecutionMode {
    Sequence,
    Graph,
}

impl QueueExecutionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sequence => "sequence",
            Self::Graph => "graph",
        }
    }
}

#[derive(Default)]
struct QueuePackage {
    run_id: Option<String>,
    execution_mode: Option<String>,
    selected_execution_host: Option<String>,
    selected_agent_command: Option<String>,
    selected_remote_launcher: Option<String>,
    selected_remote_agent_local_proxy: Option<String>,
    selected_remote_agent_remote_proxy: Option<String>,
    selected_repo_root: Option<String>,
    selected_repo_name: Option<String>,
    items: Vec<QueuePackageItem>,
}

#[derive(Clone, Default)]
struct QueuePackageItem {
    id: Option<String>,
    prompt: String,
    slug: Option<String>,
    depends_on: Vec<String>,
    repo_root: Option<String>,
    repo_name: Option<String>,
    execution_host: Option<String>,
    agent_command: Option<String>,
    task_class: Option<String>,
    remote_launcher: Option<String>,
    remote_agent_local_proxy: Option<String>,
    remote_agent_remote_proxy: Option<String>,
}

#[derive(Clone)]
struct PromptLayer {
    name: String,
    prompt: String,
}

#[derive(Serialize)]
struct QueueRunRequest {
    run_id: Option<String>,
    tab_id: Option<String>,
    execution_mode: Option<String>,
    selected_execution_host: Option<String>,
    selected_agent_command: String,
    selected_remote_launcher: Option<String>,
    selected_remote_agent_local_proxy: Option<String>,
    selected_remote_agent_remote_proxy: Option<String>,
    selected_repo_root: Option<String>,
    selected_repo_name: Option<String>,
    items: Vec<QueueRunItemRequest>,
}

#[derive(Serialize)]
struct QueueRunItemRequest {
    id: Option<String>,
    prompt: String,
    slug: Option<String>,
    depends_on: Option<Vec<String>>,
    repo_root: Option<String>,
    repo_name: Option<String>,
    execution_host: Option<String>,
    agent_command: Option<String>,
    task_class: Option<String>,
    remote_launcher: Option<String>,
    remote_agent_local_proxy: Option<String>,
    remote_agent_remote_proxy: Option<String>,
}

#[derive(Serialize)]
struct QueueAppendRequest {
    run_id: String,
    items: Vec<QueueRunItemRequest>,
}

#[derive(Serialize)]
struct QueueContinueRequest {
    run_id: String,
}

#[derive(Serialize)]
struct QueueClearRequest {
    run_id: Option<String>,
}

#[derive(Deserialize)]
struct QueueApiResponse {
    ok: bool,
    output: String,
}

struct QueueHttpClient {
    listen: String,
    auto_start: bool,
    agent: ureq::Agent,
}

include!("queue_tabs.rs");

pub(crate) fn run(args: QueueArgs) -> Result<u8> {
    match args.command {
        QueueCommand::Run(args) => run_queue(args),
        QueueCommand::Create(args) => create_queue(args),
        QueueCommand::Switch(args) => switch_queue(args),
        QueueCommand::Delete(args) => delete_queue(args),
        QueueCommand::Append(args) => append_queue(args),
        QueueCommand::List => list_queue(),
        QueueCommand::Stop(args) => stop_queue(&args),
        QueueCommand::Continue(args) => continue_queue(args),
        QueueCommand::Clear(args) => clear_queue(args),
    }
}

pub(crate) fn help_text() -> &'static str {
    QUEUE_HELP
}

fn run_queue(args: QueueRunArgs) -> Result<u8> {
    let package = load_prompt_package(&args.source)?;
    let selected_repo_root = selected_repo_root(args.repo_root.as_deref(), &package)?;
    let selected_repo_name =
        selected_repo_name(args.repo_name.as_deref(), &package, &selected_repo_root);
    let selected_agent_command = args
        .selected_agent_command
        .or(package.selected_agent_command)
        .map_or_else(default_agent_command, Ok)?;
    let request = QueueRunRequest {
        run_id: args.run_id.or(package.run_id),
        tab_id: args.tab_id,
        execution_mode: Some(
            package
                .execution_mode
                .unwrap_or_else(|| args.execution_mode.as_str().to_string()),
        ),
        selected_execution_host: package
            .selected_execution_host
            .or_else(queue_execution_host_from_env),
        selected_agent_command,
        selected_remote_launcher: package
            .selected_remote_launcher
            .or_else(queue_remote_launcher_from_env),
        selected_remote_agent_local_proxy: package
            .selected_remote_agent_local_proxy
            .or_else(queue_remote_agent_local_proxy_from_env),
        selected_remote_agent_remote_proxy: package
            .selected_remote_agent_remote_proxy
            .or_else(queue_remote_agent_remote_proxy_from_env),
        selected_repo_root: Some(selected_repo_root.display().to_string()),
        selected_repo_name,
        items: package
            .items
            .into_iter()
            .map(QueueRunItemRequest::from)
            .collect(),
    };
    let response =
        QueueHttpClient::from_args(&args.client).post_json("/api/queue/run", &request)?;
    print_queue_api_response(&response);
    Ok(0)
}

fn append_queue(args: QueueAppendArgs) -> Result<u8> {
    let package = load_prompt_package(&args.source)?;
    let package_remote_launcher = package
        .selected_remote_launcher
        .clone()
        .or_else(queue_remote_launcher_from_env);
    let package_execution_host = package
        .selected_execution_host
        .clone()
        .or_else(queue_execution_host_from_env);
    let package_remote_agent_local_proxy = package
        .selected_remote_agent_local_proxy
        .clone()
        .or_else(queue_remote_agent_local_proxy_from_env);
    let package_remote_agent_remote_proxy = package
        .selected_remote_agent_remote_proxy
        .clone()
        .or_else(queue_remote_agent_remote_proxy_from_env);
    let run_id = args
        .run_id
        .or(package.run_id)
        .map_or_else(current_queue_run_id, Ok)?;
    let items = package
        .items
        .into_iter()
        .map(|mut item| {
            if item.agent_command.is_none() {
                item.agent_command.clone_from(&args.agent_command);
            }
            if item.repo_root.is_none() {
                item.repo_root = args
                    .repo_root
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .or(package.selected_repo_root.clone());
            }
            if item.repo_name.is_none() {
                item.repo_name = args
                    .repo_name
                    .clone()
                    .or_else(|| package.selected_repo_name.clone());
            }
            if item.remote_launcher.is_none() {
                item.remote_launcher.clone_from(&package_remote_launcher);
            }
            if item.execution_host.is_none() {
                item.execution_host.clone_from(&package_execution_host);
            }
            if item.remote_agent_local_proxy.is_none() {
                item.remote_agent_local_proxy
                    .clone_from(&package_remote_agent_local_proxy);
            }
            if item.remote_agent_remote_proxy.is_none() {
                item.remote_agent_remote_proxy
                    .clone_from(&package_remote_agent_remote_proxy);
            }
            QueueRunItemRequest::from(item)
        })
        .collect::<Vec<_>>();
    let request = QueueAppendRequest { run_id, items };
    let response =
        QueueHttpClient::from_args(&args.client).post_json("/api/queue/append", &request)?;
    print_queue_api_response(&response);
    Ok(0)
}

fn list_queue() -> Result<u8> {
    let tabs = state::load_web_queue_tabs()?;
    let runs = state::load_web_queue_runs()?;
    if tabs.is_empty() && runs.is_empty() {
        println!("queue\tstatus=empty");
        return Ok(0);
    }
    let runs = runs
        .into_iter()
        .map(|(run, items)| (run.id.clone(), (run, items)))
        .collect::<BTreeMap<_, _>>();
    for tab in tabs {
        let run = tab.run_id.as_ref().and_then(|run_id| runs.get(run_id));
        if !queue_tab_visible_in_list(&tab, run) {
            continue;
        }
        let status = run.map_or("draft", |(run, _)| run.status.as_str());
        println!(
            "queue-tab\t{}\tactive={}\tdefault={}\trun={}\tstatus={}\titems={}\tlabel={}",
            compact_field(&tab.id),
            tab.active,
            tab.is_default,
            compact_field(tab.run_id.as_deref().unwrap_or("-")),
            compact_field(status),
            run.map_or(0, |(_, items)| items.len()),
            compact_field(&tab.label)
        );
        let Some((run, mut items)) = run.cloned() else {
            continue;
        };
        items.sort_by_key(|item| item.position);
        println!(
            "queue-run\t{}\tstatus={}\tmode={}\tagent={}\titems={}\tmessage={}",
            compact_field(&run.id),
            compact_field(run.status.as_str()),
            compact_field(run.execution_mode.as_str()),
            compact_field(&run.selected_agent_command),
            items.len(),
            compact_field(&run.message)
        );
        for item in items {
            let depends_on = if item.depends_on.is_empty() {
                "-".to_string()
            } else {
                item.depends_on.join(",")
            };
            println!(
                "queue-item\t{}\tid={}\tslug={}\tstatus={}\tclass={}\tagent={}\tdepends_on={}\tmessage={}",
                item.position,
                compact_field(&item.id),
                compact_field(&item.slug),
                compact_field(item.status.as_str()),
                compact_field(item.task_class.as_str()),
                compact_field(item.agent_id.as_deref().unwrap_or("-")),
                compact_field(&depends_on),
                compact_field(&item.message)
            );
        }
    }
    Ok(0)
}

fn queue_tab_visible_in_list(
    tab: &state::QueueTabRow,
    run: Option<&(state::QueueRunRow, Vec<state::QueueItemRow>)>,
) -> bool {
    run.is_some() || tab.active
}

fn stop_queue(args: &QueueClientArgs) -> Result<u8> {
    let response =
        QueueHttpClient::from_args(args).post_json("/api/queue/stop", &serde_json::json!({}))?;
    print_queue_api_response(&response);
    Ok(0)
}

fn continue_queue(args: QueueContinueArgs) -> Result<u8> {
    let run_id = args.run_id.map_or_else(current_queue_run_id, Ok)?;
    let request = QueueContinueRequest { run_id };
    let response =
        QueueHttpClient::from_args(&args.client).post_json("/api/queue/continue", &request)?;
    print_queue_api_response(&response);
    Ok(0)
}

fn clear_queue(args: QueueClearArgs) -> Result<u8> {
    let request = QueueClearRequest {
        run_id: args.run_id.or_else(|| current_queue_run_id().ok()),
    };
    let response =
        QueueHttpClient::from_args(&args.client).post_json("/api/queue/clear", &request)?;
    print_queue_api_response(&response);
    Ok(0)
}

fn print_queue_api_response(response: &QueueApiResponse) {
    if response.ok {
        println!("{}", response.output);
    } else {
        eprintln!("{}", response.output);
    }
}

fn selected_repo_root(cli_root: Option<&Path>, package: &QueuePackage) -> Result<PathBuf> {
    if let Some(root) = cli_root {
        return root
            .canonicalize()
            .with_context(|| format!("failed to resolve repository root {}", root.display()));
    }
    if let Some(root) = package.selected_repo_root.as_deref() {
        return PathBuf::from(root)
            .canonicalize()
            .with_context(|| format!("failed to resolve repository root {root}"));
    }
    Ok(repository::current_or_active()?.root)
}

fn selected_repo_name(
    cli_name: Option<&str>,
    package: &QueuePackage,
    selected_repo_root: &Path,
) -> Option<String> {
    cli_name
        .map(ToString::to_string)
        .or_else(|| package.selected_repo_name.clone())
        .or_else(|| {
            selected_repo_root
                .file_name()
                .and_then(OsStr::to_str)
                .map(ToString::to_string)
        })
}

fn default_agent_command() -> Result<String> {
    let commands = agents::available_agent_commands();
    for preferred in ["c1", "cc1", "codex"] {
        if commands.iter().any(|command| command.command == preferred) {
            return Ok(preferred.to_string());
        }
    }
    commands
        .first()
        .map(|command| command.command.clone())
        .context("no supported agent command found on PATH")
}

fn queue_remote_launcher_from_env() -> Option<String> {
    let value = std::env::var("QCOLD_QUEUE_REMOTE_LAUNCHER").ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn queue_execution_host_from_env() -> Option<String> {
    let value = std::env::var("QCOLD_QUEUE_EXECUTION_HOST").ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn queue_remote_agent_local_proxy_from_env() -> Option<String> {
    queue_non_empty_env("QCOLD_QUEUE_REMOTE_AGENT_LOCAL_PROXY")
}

fn queue_remote_agent_remote_proxy_from_env() -> Option<String> {
    queue_non_empty_env("QCOLD_QUEUE_REMOTE_AGENT_REMOTE_PROXY")
}

fn queue_non_empty_env(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn current_queue_run_id() -> Result<String> {
    let (run, _) = state::load_web_queue()?;
    run.map(|run| run.id)
        .context("no current queue run; pass an explicit run id")
}

fn compact_field(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(400)
        .collect()
}

impl From<QueuePackageItem> for QueueRunItemRequest {
    fn from(item: QueuePackageItem) -> Self {
        Self {
            id: item.id,
            prompt: item.prompt,
            slug: item.slug,
            depends_on: (!item.depends_on.is_empty()).then_some(item.depends_on),
            repo_root: item.repo_root,
            repo_name: item.repo_name,
            execution_host: item.execution_host,
            agent_command: item.agent_command,
            task_class: item.task_class,
            remote_launcher: item.remote_launcher,
            remote_agent_local_proxy: item.remote_agent_local_proxy,
            remote_agent_remote_proxy: item.remote_agent_remote_proxy,
        }
    }
}

impl QueueHttpClient {
    fn from_args(args: &QueueClientArgs) -> Self {
        Self {
            listen: args.listen.clone(),
            auto_start: !args.no_start_daemon,
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(5))
                .build(),
        }
    }

    fn post_json<T>(&self, path: &str, payload: &T) -> Result<QueueApiResponse>
    where
        T: Serialize,
    {
        self.ensure_daemon()?;
        let url = self.url(path);
        let mut request = self.agent.post(&url);
        if let Some(token) = std::env::var("QCOLD_WEBAPP_WRITE_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            request = request.set("X-QCOLD-Write-Token", &token);
        }
        match request.send_json(serde_json::to_value(payload)?) {
            Ok(response) => response
                .into_json()
                .with_context(|| format!("failed to decode queue API response from {url}")),
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_string().unwrap_or_default();
                bail!("queue API {path} failed with HTTP {code}: {body}");
            }
            Err(err) => Err(err).with_context(|| format!("queue API request failed: {url}")),
        }
    }

    fn ensure_daemon(&self) -> Result<()> {
        if self.healthz() {
            return Ok(());
        }
        if !self.auto_start {
            bail!(
                "Q-COLD dashboard daemon is not reachable at {}; run `qcold telegram serve \
                 --listen {} --daemon`",
                self.base_url(),
                self.listen
            );
        }
        webapp::start_daemon_for_listen(self.daemon_listen_addr())?;
        for _ in 0..20 {
            if self.healthz() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(250));
        }
        bail!(
            "Q-COLD dashboard daemon did not become reachable at {}",
            self.base_url()
        );
    }

    fn healthz(&self) -> bool {
        self.agent
            .get(&self.url("/healthz"))
            .call()
            .is_ok_and(|response| response.status() == 200)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url(), path)
    }

    fn base_url(&self) -> String {
        if self.listen.starts_with("http://") || self.listen.starts_with("https://") {
            self.listen.trim_end_matches('/').to_string()
        } else {
            format!("http://{}", self.listen.trim_end_matches('/'))
        }
    }

    fn daemon_listen_addr(&self) -> &str {
        self.listen
            .strip_prefix("http://")
            .or_else(|| self.listen.strip_prefix("https://"))
            .unwrap_or(&self.listen)
            .trim_end_matches('/')
    }
}

fn load_prompt_package(source: &QueuePromptSourceArgs) -> Result<QueuePackage> {
    let mut package = source
        .from
        .as_deref()
        .map(load_prompt_package_path)
        .transpose()?
        .unwrap_or_default();
    if source.slug.is_some() && source.prompts.len() != 1 {
        bail!("--slug can only be used with exactly one --prompt");
    }
    for prompt in &source.prompts {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            continue;
        }
        package.items.push(QueuePackageItem {
            prompt: prompt.to_string(),
            slug: source.slug.clone(),
            ..QueuePackageItem::default()
        });
    }
    if package.items.is_empty() {
        bail!("queue package has no prompt items; pass --from <path> or --prompt <text>");
    }
    Ok(package)
}

fn load_prompt_package_path(path: &Path) -> Result<QueuePackage> {
    if path.is_dir() {
        return load_directory_package(path);
    }
    if is_zip_path(path) {
        return load_zip_package(path);
    }
    let content = read_prompt_file(path)?;
    if is_json_path(path) {
        return parse_json_package(&content, &path.display().to_string());
    }
    Ok(package_from_single_prompt(
        content,
        slug_from_path(path).map(ToString::to_string),
    ))
}

fn load_directory_package(root: &Path) -> Result<QueuePackage> {
    for manifest in ["queue.json", "qcold-queue.json"] {
        let path = root.join(manifest);
        if path.is_file() {
            let content = read_prompt_file(&path)?;
            return parse_json_package(&content, &path.display().to_string());
        }
    }
    let files = collect_prompt_files(root)?;
    package_from_named_files(files)
}

fn load_zip_package(path: &Path) -> Result<QueuePackage> {
    let entries = zip_entries(path)?;
    for manifest in ["queue.json", "qcold-queue.json"] {
        if entries.iter().any(|entry| entry == manifest) {
            let content = zip_entry_content(path, manifest)?;
            return parse_json_package(&content, &format!("{}:{manifest}", path.display()));
        }
    }
    let mut files = Vec::new();
    for entry in entries {
        if supported_prompt_path(Path::new(&entry)) {
            files.push((entry.clone(), zip_entry_content(path, &entry)?));
        }
    }
    package_from_named_files(files)
}

fn package_from_single_prompt(prompt: String, slug: Option<String>) -> QueuePackage {
    QueuePackage {
        items: vec![QueuePackageItem {
            prompt,
            slug,
            ..QueuePackageItem::default()
        }],
        ..QueuePackage::default()
    }
}

fn package_from_named_files(files: Vec<(String, String)>) -> Result<QueuePackage> {
    if files.len() > MAX_PROMPT_PACKAGE_FILES {
        bail!(
            "queue package has {} prompt-like files; limit is {}",
            files.len(),
            MAX_PROMPT_PACKAGE_FILES
        );
    }
    let mut layers = Vec::new();
    let mut explicit_items = Vec::new();
    let mut fallback_items = Vec::new();
    for (name, content) in files {
        if content.trim().is_empty() {
            continue;
        }
        match queue_file_role(&name) {
            QueueFileRole::Layer => layers.push(PromptLayer {
                name: slug_from_str(&name).unwrap_or_else(|| format!("layer-{}", layers.len() + 1)),
                prompt: content,
            }),
            QueueFileRole::Prompt => explicit_items.push((name, content)),
            QueueFileRole::Fallback => fallback_items.push((name, content)),
        }
    }
    let source_items = if explicit_items.is_empty() {
        fallback_items
    } else {
        explicit_items
    };
    let items = source_items
        .into_iter()
        .map(|(name, content)| QueuePackageItem {
            prompt: apply_layers(&content, &layers, &layer_names(&layers)),
            slug: slug_from_str(&name),
            ..QueuePackageItem::default()
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        bail!("queue package has no supported prompt files");
    }
    Ok(QueuePackage {
        items,
        ..QueuePackage::default()
    })
}

enum QueueFileRole {
    Layer,
    Prompt,
    Fallback,
}

fn queue_file_role(name: &str) -> QueueFileRole {
    let normalized = name.replace('\\', "/");
    let mut parts = normalized.split('/');
    match parts.next() {
        Some("layers") => QueueFileRole::Layer,
        Some("prompts" | "tasks") => QueueFileRole::Prompt,
        _ => QueueFileRole::Fallback,
    }
}

fn collect_prompt_files(root: &Path) -> Result<Vec<(String, String)>> {
    let mut paths = Vec::new();
    collect_prompt_file_paths(root, root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|relative| {
            let path = root.join(&relative);
            let content = read_prompt_file(&path)?;
            Ok((relative.to_string_lossy().replace('\\', "/"), content))
        })
        .collect()
}

fn collect_prompt_file_paths(root: &Path, dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_prompt_file_paths(root, &path, paths)?;
        } else if file_type.is_file() && supported_prompt_path(&path) {
            paths.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn parse_json_package(content: &str, label: &str) -> Result<QueuePackage> {
    let value = serde_json::from_str::<Value>(content)
        .with_context(|| format!("failed to parse queue JSON package {label}"))?;
    package_from_json_value(value)
}

fn package_from_json_value(value: Value) -> Result<QueuePackage> {
    match value {
        Value::Array(values) => {
            let layers = Vec::new();
            Ok(QueuePackage {
                items: json_items(&values, &layers, &[])?,
                ..QueuePackage::default()
            })
        }
        Value::Object(object) => {
            let layers = json_layers(object.get("layers"))?;
            let default_layers = object
                .get("default_layers")
                .map(json_string_array)
                .transpose()?
                .unwrap_or_else(|| layer_names(&layers));
            let items = match object.get("items") {
                Some(Value::Array(values)) => json_items(values, &layers, &default_layers)?,
                Some(_) => bail!("queue JSON `items` must be an array"),
                None if object.get("prompt").is_some() => {
                    vec![json_item(
                        &Value::Object(object.clone()),
                        &layers,
                        &default_layers,
                    )?]
                }
                None => bail!("queue JSON package must contain `items` or `prompt`"),
            };
            Ok(QueuePackage {
                run_id: json_string_field(&object, "run_id"),
                execution_mode: json_string_field(&object, "execution_mode"),
                selected_execution_host: json_string_field(&object, "selected_execution_host")
                    .or_else(|| json_string_field(&object, "execution_host")),
                selected_agent_command: json_string_field(&object, "selected_agent_command")
                    .or_else(|| json_string_field(&object, "agent_command")),
                selected_remote_launcher: json_string_field(&object, "selected_remote_launcher")
                    .or_else(|| json_string_field(&object, "remote_launcher")),
                selected_remote_agent_local_proxy: json_string_field(
                    &object,
                    "selected_remote_agent_local_proxy",
                )
                .or_else(|| json_string_field(&object, "remote_agent_local_proxy")),
                selected_remote_agent_remote_proxy: json_string_field(
                    &object,
                    "selected_remote_agent_remote_proxy",
                )
                .or_else(|| json_string_field(&object, "remote_agent_remote_proxy")),
                selected_repo_root: json_string_field(&object, "selected_repo_root")
                    .or_else(|| json_string_field(&object, "repo_root")),
                selected_repo_name: json_string_field(&object, "selected_repo_name")
                    .or_else(|| json_string_field(&object, "repo_name")),
                items,
            })
        }
        Value::String(prompt) => Ok(package_from_single_prompt(prompt, None)),
        _ => bail!("queue JSON package must be an object, array, or string"),
    }
}

fn json_items(
    values: &[Value],
    layers: &[PromptLayer],
    default_layers: &[String],
) -> Result<Vec<QueuePackageItem>> {
    values
        .iter()
        .map(|value| json_item(value, layers, default_layers))
        .collect()
}

fn json_item(
    value: &Value,
    layers: &[PromptLayer],
    default_layers: &[String],
) -> Result<QueuePackageItem> {
    match value {
        Value::String(prompt) => Ok(QueuePackageItem {
            prompt: apply_layers(prompt, layers, default_layers),
            ..QueuePackageItem::default()
        }),
        Value::Object(object) => {
            let prompt =
                json_string_field(object, "prompt").context("queue item is missing prompt")?;
            let item_layers = object
                .get("layers")
                .map(json_string_array)
                .transpose()?
                .unwrap_or_else(|| default_layers.to_vec());
            Ok(QueuePackageItem {
                id: json_string_field(object, "id"),
                prompt: apply_layers(&prompt, layers, &item_layers),
                slug: json_string_field(object, "slug"),
                depends_on: object
                    .get("depends_on")
                    .map(json_string_array)
                    .transpose()?
                    .unwrap_or_default(),
                repo_root: json_string_field(object, "repo_root"),
                repo_name: json_string_field(object, "repo_name"),
                execution_host: json_string_field(object, "execution_host"),
                agent_command: json_string_field(object, "agent_command"),
                task_class: json_string_field(object, "task_class")
                    .or_else(|| json_string_field(object, "class")),
                remote_launcher: json_string_field(object, "remote_launcher"),
                remote_agent_local_proxy: json_string_field(object, "remote_agent_local_proxy"),
                remote_agent_remote_proxy: json_string_field(object, "remote_agent_remote_proxy"),
            })
        }
        _ => bail!("queue item must be a string or object"),
    }
}

fn json_layers(value: Option<&Value>) -> Result<Vec<PromptLayer>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    match value {
        Value::Array(values) => values
            .iter()
            .enumerate()
            .map(|(index, value)| json_layer(index, value))
            .collect(),
        Value::Object(object) => object
            .iter()
            .map(|(name, value)| {
                let prompt = match value {
                    Value::String(prompt) => prompt.clone(),
                    Value::Object(layer) => json_string_field(layer, "prompt")
                        .with_context(|| format!("layer {name} is missing prompt"))?,
                    _ => bail!("layer {name} must be a string or object"),
                };
                Ok(PromptLayer {
                    name: name.clone(),
                    prompt,
                })
            })
            .collect(),
        _ => bail!("queue JSON `layers` must be an array or object"),
    }
}

fn json_layer(index: usize, value: &Value) -> Result<PromptLayer> {
    match value {
        Value::String(prompt) => Ok(PromptLayer {
            name: format!("layer-{}", index + 1),
            prompt: prompt.clone(),
        }),
        Value::Object(object) => Ok(PromptLayer {
            name: json_string_field(object, "name")
                .unwrap_or_else(|| format!("layer-{}", index + 1)),
            prompt: json_string_field(object, "prompt").context("layer is missing prompt")?,
        }),
        _ => bail!("queue JSON layer must be a string or object"),
    }
}

fn apply_layers(prompt: &str, layers: &[PromptLayer], selected: &[String]) -> String {
    let by_name = layers
        .iter()
        .map(|layer| (layer.name.as_str(), layer.prompt.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut parts = Vec::new();
    let mut seen = HashSet::new();
    for name in selected {
        if seen.insert(name.as_str()) {
            if let Some(layer_prompt) = by_name.get(name.as_str()) {
                parts.push(format!("[layer:{name}]\n{}", layer_prompt.trim()));
            }
        }
    }
    parts.push(prompt.trim().to_string());
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn layer_names(layers: &[PromptLayer]) -> Vec<String> {
    layers.iter().map(|layer| layer.name.clone()).collect()
}

fn json_string_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_string_array(value: &Value) -> Result<Vec<String>> {
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(ToString::to_string)
                    .context("expected a non-empty string")
            })
            .collect(),
        Value::String(value) => Ok(vec![value.trim().to_string()]),
        _ => bail!("expected string array"),
    }
}

fn zip_entries(path: &Path) -> Result<Vec<String>> {
    let output = Command::new("unzip")
        .arg("-Z1")
        .arg(path)
        .output()
        .with_context(|| format!("failed to list ZIP package {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "unzip -Z1 failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.ends_with('/'))
        .map(ToString::to_string)
        .collect())
}

fn zip_entry_content(path: &Path, entry: &str) -> Result<String> {
    let output = Command::new("unzip")
        .arg("-p")
        .arg(path)
        .arg(entry)
        .output()
        .with_context(|| format!("failed to read {entry} from {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "unzip -p failed for {}:{}: {}",
            path.display(),
            entry,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).with_context(|| format!("ZIP entry is not UTF-8: {entry}"))
}

fn read_prompt_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

fn supported_prompt_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "txt" | "prompt" | "qprompt"
            )
        })
}

fn is_json_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

fn slug_from_path(path: &Path) -> Option<&str> {
    path.file_stem().and_then(OsStr::to_str)
}

fn slug_from_str(name: &str) -> Option<String> {
    Path::new(name)
        .file_stem()
        .and_then(OsStr::to_str)
        .map(ToString::to_string)
}
