const TERMINAL_CAPTURE_LINES: usize = 2_000;

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
        "/agent_start --cwd {cwd} <track> :: codex exec \"Use the launched host-side agent \
         workspace as your home base for {root}; do not enter a devcontainer from \
         $QCOLD_AGENT_WORKTREE. Start managed task <slug> with cargo qcold task open <slug>, enter \
         that managed task worktree and its devcontainer if the task flow provides one, reread \
         AGENTS.md and task logs, then do: <task>. Shape broad searches before reading raw output; \
         use qcold guard -- <command> for risky commands. Drive the task to terminal closeout unless \
         a business or external blocker requires task pause or blocked closeout. After closeout, cd \
         back to $QCOLD_AGENT_WORKTREE before starting a new chat or task.\"",
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
                .map_or_else(|| generated_agent_label(&context), ToString::to_string);
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

#[cfg(test)]
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

#[cfg(test)]
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
            "#{session_name}\t#{window_index}.#{pane_index}\t#{pane_pid}\t\
             #{pane_current_command}\t#{pane_current_path}",
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
            "--full",
            "--pane-id",
            pane,
        ])
        .output()
        .with_context(|| format!("failed to dump zellij pane {session}:{pane}"))?;
    if !output.status.success() {
        bail!("zellij dump-screen failed with {}", output.status);
    }
    Ok(trim_terminal_scrollback(&String::from_utf8_lossy(&output.stdout)))
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
    let capture_start = terminal_capture_start_arg();
    let output = Command::new("tmux")
        .args([
            "capture-pane",
            "-p",
            "-e",
            "-J",
            "-S",
            &capture_start,
            "-t",
            target,
        ])
        .output()
        .with_context(|| format!("failed to capture tmux pane {target}"))?;
    if !output.status.success() {
        bail!("tmux capture-pane failed with {}", output.status);
    }
    Ok(trim_terminal_scrollback(&String::from_utf8_lossy(&output.stdout)))
}

fn terminal_capture_start_arg() -> String {
    format!("-{TERMINAL_CAPTURE_LINES}")
}

fn trim_terminal_scrollback(output: &str) -> String {
    let trimmed = output.trim_end();
    let line_count = trimmed.lines().count();
    if line_count <= TERMINAL_CAPTURE_LINES {
        return trimmed.to_string();
    }
    trimmed
        .lines()
        .skip(line_count - TERMINAL_CAPTURE_LINES)
        .collect::<Vec<_>>()
        .join("\n")
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
        return Some("web-daemon".to_string());
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
