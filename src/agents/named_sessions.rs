#[derive(Args)]
struct NamedSessionsArgs {
    #[command(subcommand)]
    command: NamedSessionsCommand,
}

#[derive(Subcommand)]
enum NamedSessionsCommand {
    #[command(about = "List named Codex sessions known to Q-COLD")]
    List(NamedSessionListArgs),
    #[command(about = "Drop named Codex sessions by display name")]
    Drop(NamedSessionDropArgs),
    #[command(about = "Drop all named Codex sessions in a scope")]
    DropAll(NamedSessionDropAllArgs),
}

#[derive(Args)]
struct NamedSessionListArgs {
    #[command(flatten)]
    scope: NamedSessionScopeArgs,
    #[arg(long, help = "Limit to one terminal display name")]
    name: Option<String>,
}

#[derive(Args)]
struct NamedSessionDropArgs {
    #[command(flatten)]
    scope: NamedSessionScopeArgs,
    #[arg(long, help = "Terminal display name to drop")]
    name: String,
    #[arg(long, help = "Show matching sessions without deleting records")]
    dry_run: bool,
    #[arg(long, help = "Also terminate and drop still-running named terminals")]
    include_running: bool,
}

#[derive(Args)]
struct NamedSessionDropAllArgs {
    #[command(flatten)]
    scope: NamedSessionScopeArgs,
    #[arg(long, help = "Allow dropping every named Codex session across all scopes")]
    all: bool,
    #[arg(long, help = "Show matching sessions without deleting records")]
    dry_run: bool,
    #[arg(long, help = "Also terminate and drop still-running named terminals")]
    include_running: bool,
}

#[derive(Args, Clone, Default)]
struct NamedSessionScopeArgs {
    #[arg(long, help = "Limit to a Codex-like command/account, such as cc1 or cc2")]
    agent: Option<String>,
    #[arg(long, help = "Limit to one Q-COLD agent track, such as c1 or c2")]
    track: Option<String>,
    #[arg(long, help = "Limit to one Codex account key, such as 1, 2, or default")]
    account: Option<String>,
    #[arg(long, help = "Limit to sessions rooted in one primary repository")]
    repo_root: Option<PathBuf>,
}

#[derive(Clone, Default)]
struct NamedSessionFilter {
    track: Option<String>,
    account: Option<String>,
    name: Option<String>,
    repo_root: Option<PathBuf>,
}

struct NamedSessionRecord {
    agent_id: String,
    track: String,
    account: String,
    name: String,
    target: String,
    state: &'static str,
    resume_state: &'static str,
    started_at: u64,
    command: String,
    cwd: Option<PathBuf>,
    primary_root: Option<PathBuf>,
    session_id: Option<String>,
    exit_status: Option<i32>,
}

struct NamedSessionDropSummary {
    dry_run: bool,
    matched: usize,
    dropped: usize,
    skipped_running: usize,
    deleted_task_records: usize,
    deleted_agents: usize,
    deleted_metadata: usize,
    deleted_logs: usize,
    events: Vec<NamedSessionDropEvent>,
}

struct NamedSessionDropEvent {
    action: &'static str,
    agent_id: String,
    name: String,
    track: String,
    account: String,
}

#[derive(Default)]
struct NamedSessionDeletion {
    task_records: usize,
    agent: usize,
    metadata: usize,
    logs: usize,
}

fn run_named_sessions(args: NamedSessionsArgs) -> Result<()> {
    match args.command {
        NamedSessionsCommand::List(args) => {
            let filter = args.scope.to_filter(args.name.as_deref())?;
            print!("{}", render_named_sessions(&named_session_rows(&filter)?));
        }
        NamedSessionsCommand::Drop(args) => {
            let filter = args.scope.to_filter(Some(&args.name))?;
            let summary = drop_named_sessions(&filter, args.dry_run, args.include_running)?;
            print!("{}", summary.render());
        }
        NamedSessionsCommand::DropAll(args) => {
            if !args.all && !args.scope.has_scope() {
                bail!(
                    "drop-all requires a scope such as --agent cc1, or pass --all to drop every \
                     named Codex session"
                );
            }
            let filter = args.scope.to_filter(None)?;
            let summary = drop_named_sessions(&filter, args.dry_run, args.include_running)?;
            print!("{}", summary.render());
        }
    }
    Ok(())
}

impl NamedSessionScopeArgs {
    fn has_scope(&self) -> bool {
        self.agent.is_some()
            || self.track.is_some()
            || self.account.is_some()
            || self.repo_root.is_some()
    }

    fn to_filter(&self, name: Option<&str>) -> Result<NamedSessionFilter> {
        let agent = self.agent.as_deref().map(normalized_agent_command).transpose()?;
        let agent_account = agent.as_deref().map(agent_account_key);
        if let (Some(account), Some(agent_account)) = (self.account.as_deref(), agent_account.as_deref()) {
            if account != agent_account {
                bail!("--account {account:?} does not match --agent account {agent_account:?}");
            }
        }
        let track = self
            .track
            .clone()
            .or_else(|| agent.as_deref().and_then(default_track_for_agent_command));
        let account = self.account.clone().or(agent_account);
        Ok(NamedSessionFilter {
            track: clean_optional_filter(track),
            account: clean_optional_filter(account),
            name: name.map(normalize_display_name).filter(|value| !value.is_empty()),
            repo_root: canonical_repo_root_arg(self.repo_root.as_deref())?,
        })
    }
}

impl NamedSessionDropSummary {
    fn render(&self) -> String {
        let mut lines = vec![format!(
            "named-session-drop\tmatched={}\tdry_run={}\tdropped={}\tskipped_running={}\
             \tdeleted_agents={}\tdeleted_task_records={}\tdeleted_metadata={}\tdeleted_logs={}",
            self.matched,
            self.dry_run,
            self.dropped,
            self.skipped_running,
            self.deleted_agents,
            self.deleted_task_records,
            self.deleted_metadata,
            self.deleted_logs
        )];
        lines.extend(self.events.iter().map(NamedSessionDropEvent::render));
        format!("{}\n", lines.join("\n"))
    }
}

impl NamedSessionDropEvent {
    fn render(&self) -> String {
        format!(
            "named-session-drop-event\taction={}\tname={}\ttrack={}\taccount={}\tagent={}",
            self.action,
            named_session_field(&self.name),
            named_session_field(&self.track),
            named_session_field(&self.account),
            named_session_field(&self.agent_id)
        )
    }
}

fn render_named_sessions(rows: &[NamedSessionRecord]) -> String {
    let mut lines = vec![format!("named-sessions\tcount={}", rows.len())];
    lines.extend(rows.iter().map(render_named_session));
    format!("{}\n", lines.join("\n"))
}

fn render_named_session(row: &NamedSessionRecord) -> String {
    let mut line = format!(
        "named-session\tname={}\ttrack={}\taccount={}\tstate={}\tresume={}\tagent={}\
         \tstarted_at={}\ttarget={}",
        named_session_field(&row.name),
        named_session_field(&row.track),
        named_session_field(&row.account),
        row.state,
        row.resume_state,
        named_session_field(&row.agent_id),
        row.started_at,
        named_session_field(&row.target)
    );
    if let Some(status) = row.exit_status {
        let _ = write!(line, "\texit_status={status}");
    }
    if let Some(session_id) = &row.session_id {
        let _ = write!(line, "\tsession={}", named_session_field(session_id));
    }
    if let Some(root) = &row.primary_root {
        let _ = write!(line, "\trepo_root={}", named_session_field(&root.display().to_string()));
    }
    if let Some(cwd) = &row.cwd {
        let _ = write!(line, "\tcwd={}", named_session_field(&cwd.display().to_string()));
    }
    let _ = write!(line, "\tcmd={}", named_session_field(&row.command));
    line
}

fn named_session_rows(filter: &NamedSessionFilter) -> Result<Vec<NamedSessionRecord>> {
    let _ = crate::sync_codex_task_records();
    let metadata = terminal_metadata_by_target()?;
    let tasks = state::load_task_records(None, 1000)?;
    let mut rows = Vec::new();
    for record in AgentState::load()?.records {
        let Some(target) = terminal_target_key(&record) else {
            continue;
        };
        let Some(name) = terminal_display_name(&record, &metadata).map(ToString::to_string) else {
            continue;
        };
        let command = terminal_command_from_record(&record.command);
        let Some(account) = codex_account_from_command(&command) else {
            continue;
        };
        if !filter.matches_basic(&record.track, &account, &name) {
            continue;
        }
        let primary_root = record
            .cwd
            .as_deref()
            .map(named_session_primary_root)
            .transpose()?
            .flatten();
        if !filter.matches_repo(primary_root.as_deref()) {
            continue;
        }
        let state = process_state(record.pid);
        let session_id = task_resume_session_for_agent(&tasks, &record.id);
        let exit_status = terminal_exit_status(&record.id);
        let resume_state = named_session_resume_state(state, exit_status, session_id.as_deref());
        rows.push(NamedSessionRecord {
            agent_id: record.id,
            track: record.track,
            account,
            name,
            target,
            state,
            resume_state,
            started_at: record.started_at,
            command,
            cwd: record.cwd,
            primary_root,
            session_id,
            exit_status,
        });
    }
    rows.sort_by_key(|row| (std::cmp::Reverse(row.started_at), row.name.clone(), row.agent_id.clone()));
    Ok(rows)
}

impl NamedSessionFilter {
    fn matches_basic(&self, track: &str, account: &str, name: &str) -> bool {
        self.track.as_deref().is_none_or(|wanted| wanted == track)
            && self.account.as_deref().is_none_or(|wanted| wanted == account)
            && self
                .name
                .as_deref()
                .is_none_or(|wanted| wanted == normalize_display_name(name))
    }

    fn matches_repo(&self, primary_root: Option<&Path>) -> bool {
        self.repo_root.as_deref().is_none_or(|wanted| primary_root == Some(wanted))
    }
}

fn drop_named_sessions(
    filter: &NamedSessionFilter,
    dry_run: bool,
    include_running: bool,
) -> Result<NamedSessionDropSummary> {
    let rows = named_session_rows(filter)?;
    let mut summary = NamedSessionDropSummary {
        dry_run,
        matched: rows.len(),
        dropped: 0,
        skipped_running: 0,
        deleted_task_records: 0,
        deleted_agents: 0,
        deleted_metadata: 0,
        deleted_logs: 0,
        events: Vec::new(),
    };
    for row in rows {
        let running = row.state == "running";
        if running && !include_running {
            summary.skipped_running += 1;
            summary.events.push(row.drop_event("skipped-running"));
            continue;
        }
        if dry_run {
            summary.events.push(row.drop_event("dry-run"));
            continue;
        }
        if running {
            terminate_terminal_target_for_key(&row.target)?;
        }
        let deletion = delete_named_session_binding(&row.agent_id, &row.target)?;
        summary.deleted_task_records += deletion.task_records;
        summary.deleted_agents += deletion.agent;
        summary.deleted_metadata += deletion.metadata;
        summary.deleted_logs += deletion.logs;
        summary.dropped += 1;
        summary.events.push(row.drop_event("dropped"));
    }
    Ok(summary)
}

impl NamedSessionRecord {
    fn drop_event(&self, action: &'static str) -> NamedSessionDropEvent {
        NamedSessionDropEvent {
            action,
            agent_id: self.agent_id.clone(),
            name: self.name.clone(),
            track: self.track.clone(),
            account: self.account.clone(),
        }
    }
}

fn terminate_terminal_target_for_key(target: &str) -> Result<()> {
    if let Some((session, pane)) = parse_zellij_target(target) {
        return terminate_terminal_target(&TerminalTarget::Zellij { session, pane });
    }
    let session = target.split(':').next().unwrap_or(target).to_string();
    terminate_terminal_target(&TerminalTarget::Tmux { session })
}

fn parse_zellij_target(target: &str) -> Option<(String, String)> {
    let rest = target.strip_prefix("zellij:")?;
    let (session, pane) = rest.rsplit_once(':')?;
    Some((session.to_string(), pane.to_string()))
}

fn delete_named_session_binding(agent_id: &str, target: &str) -> Result<NamedSessionDeletion> {
    let task_records = state::delete_ad_hoc_task_records_for_agent(agent_id)?;
    let agent = usize::from(state::delete_agent_record(agent_id)?);
    state::save_terminal_metadata(target, None, None)?;
    let logs = delete_named_session_logs(agent_id)?;
    Ok(NamedSessionDeletion {
        task_records,
        agent,
        metadata: 1,
        logs,
    })
}

fn named_session_primary_root(cwd: &Path) -> Result<Option<PathBuf>> {
    if !cwd.is_dir() {
        return Ok(None);
    }
    if let Some((_, primary_root)) = agent_worktree_primary_for_cwd(cwd)? {
        return Ok(Some(primary_root));
    }
    Ok(git_root_for(cwd)
        .ok()
        .map(|root| root.canonicalize().unwrap_or(root)))
}

fn named_session_resume_state(
    process_state: &str,
    exit_status: Option<i32>,
    session_id: Option<&str>,
) -> &'static str {
    if process_state == "running" {
        "running"
    } else if exit_status == Some(0) {
        "closed"
    } else if session_id.is_some() {
        "resumable"
    } else {
        "exited"
    }
}

fn canonical_repo_root_arg(path: Option<&Path>) -> Result<Option<PathBuf>> {
    let Some(path) = path else {
        return Ok(None);
    };
    Ok(Some(
        git_root_for(path)?
            .canonicalize()
            .with_context(|| format!("failed to resolve repo root {}", path.display()))?,
    ))
}

fn normalized_agent_command(command: &str) -> Result<String> {
    let command = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command)
        .to_string();
    if !is_codex_agent_command(&command) {
        bail!("unsupported Codex agent command for --agent: {command}");
    }
    Ok(command)
}

fn default_track_for_agent_command(command: &str) -> Option<String> {
    match command {
        "c1" | "cc1" => Some("c1".to_string()),
        "c2" | "cc2" => Some("c2".to_string()),
        command if command.starts_with("codex") => Some(command.to_string()),
        _ => None,
    }
}

fn clean_optional_filter(value: Option<String>) -> Option<String> {
    value.map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
}

fn delete_named_session_logs(id: &str) -> Result<usize> {
    let mut deleted = 0;
    let paths = [
        log_path(id, "out")?,
        log_path(id, "err")?,
        terminal_exit_status_path(id)?,
        state_dir()?.join("logs").join(format!("{id}.zellij.kdl")),
    ];
    for path in paths {
        match fs::remove_file(&path) {
            Ok(()) => deleted += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("failed to delete {}", path.display())),
        }
    }
    Ok(deleted)
}

fn named_session_field(value: &str) -> String {
    value
        .replace('\t', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(400)
        .collect()
}
