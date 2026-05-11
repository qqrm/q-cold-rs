fn terminal_target(record: &AgentRecord) -> Option<TerminalTarget> {
    match record.command.as_slice() {
        [tmux, new_session, flag, session, ..]
            if tmux == "tmux" && new_session == "new-session" && flag == "-s" =>
        {
            Some(TerminalTarget::Tmux {
                session: session.clone(),
            })
        }
        [zellij, session_flag, session, pane_marker, pane, ..]
            if zellij == "zellij" && session_flag == "--session" && pane_marker == "pane" =>
        {
            Some(TerminalTarget::Zellij {
                session: session.clone(),
                pane: pane.clone(),
            })
        }
        _ => None,
    }
}

fn terminal_command_from_record(command: &[String]) -> String {
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

fn render_record(
    record: &AgentRecord,
    metadata: &HashMap<String, state::TerminalMetadataRow>,
) -> String {
    render_record_with_state(record, metadata, process_state(record.pid))
}

fn render_record_with_state(
    record: &AgentRecord,
    metadata: &HashMap<String, state::TerminalMetadataRow>,
    state: &str,
) -> String {
    let mut line = format!(
        "agent\t{}\ttrack={}\tpid={}\tstate={}\tstarted_at={}\tcmd={}",
        record.id,
        record.track,
        record.pid,
        state,
        record.started_at,
        record.command.join(" ")
    );
    if let Some(cwd) = &record.cwd {
        let _ = write!(line, "\tcwd={}", cwd.display());
    }
    if let Some(name) = terminal_display_name(record, metadata) {
        let _ = write!(line, "\tname={name}");
    }
    if let Some(target) = terminal_target(record) {
        match target {
            TerminalTarget::Tmux { session } => {
                let _ = write!(
                    line,
                    "\tterminal={session}\ttarget={session}:0.0\tattach=tmux attach-session -t {session}"
                );
            }
            TerminalTarget::Zellij { session, pane } => {
                let _ = write!(
                    line,
                    "\tterminal={session}\ttarget=zellij:{session}:{pane}\tattach=zellij attach {session}"
                );
            }
        }
    }
    line
}

fn attach_tracked_terminal(selector: &str) -> Result<()> {
    let state = AgentState::load()?;
    let metadata = terminal_metadata_by_target().unwrap_or_default();
    let matches = terminal_attach_matches(&state.records, &metadata, selector);
    match matches.as_slice() {
        [record] => attach_terminal(record),
        [] => bail!(
            "no attachable terminal agent matches {selector:?}\n{}",
            terminal_attach_candidates(&state.records, &metadata)
        ),
        records => bail!(
            "terminal selector {selector:?} is ambiguous; matched {}\n{}",
            records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            terminal_attach_candidates(&state.records, &metadata)
        ),
    }
}

fn terminal_attach_matches<'a>(
    records: &'a [AgentRecord],
    metadata: &HashMap<String, state::TerminalMetadataRow>,
    selector: &str,
) -> Vec<&'a AgentRecord> {
    let selector = selector.trim();
    let normalized = normalize_display_name(selector);
    let mut matches = Vec::new();
    for record in records {
        let exact_match = terminal_attach_keys(record, metadata)
            .iter()
            .any(|key| key == selector);
        let name_match = terminal_display_name(record, metadata)
            .is_some_and(|name| normalize_display_name(name) == normalized);
        if exact_match || name_match {
            matches.push(record);
        }
    }
    matches
}

fn terminal_attach_keys(
    record: &AgentRecord,
    metadata: &HashMap<String, state::TerminalMetadataRow>,
) -> Vec<String> {
    let mut keys = vec![record.id.clone()];
    match terminal_target(record) {
        Some(TerminalTarget::Tmux { session }) => {
            keys.push(session.clone());
            keys.push(format!("{session}:0.0"));
        }
        Some(TerminalTarget::Zellij { session, pane }) => {
            keys.push(session.clone());
            keys.push(format!("zellij:{session}:{pane}"));
        }
        None => return Vec::new(),
    }
    if let Some(name) = terminal_display_name(record, metadata) {
        keys.push(name.to_string());
    }
    keys
}

fn terminal_attach_candidates(
    records: &[AgentRecord],
    metadata: &HashMap<String, state::TerminalMetadataRow>,
) -> String {
    let mut candidates = records
        .iter()
        .filter_map(|record| {
            let target = terminal_target_key(record)?;
            let name = terminal_display_name(record, metadata).unwrap_or("-");
            Some(format!(
                "  {} name={} target={} attach=qcold agent attach {}",
                record.id, name, target, record.id
            ))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        "attachable agents: none".to_string()
    } else {
        candidates.sort();
        format!("attachable agents:\n{}", candidates.join("\n"))
    }
}

fn assign_terminal_display_name(record: &AgentRecord) -> Result<()> {
    let Some(target) = terminal_target_key(record) else {
        return Ok(());
    };
    let metadata = terminal_metadata_by_target()?;
    if metadata
        .get(&target)
        .and_then(|metadata| metadata.name.as_deref())
        .is_some_and(|name| !name.trim().is_empty())
    {
        return Ok(());
    }
    let used = used_terminal_display_names(&metadata)?;
    let name = choose_agent_display_name(&record.id, &used);
    state::save_terminal_metadata(&target, Some(&name), None)
}

fn terminal_display_name<'a>(
    record: &AgentRecord,
    metadata: &'a HashMap<String, state::TerminalMetadataRow>,
) -> Option<&'a str> {
    let target = terminal_target_key(record)?;
    metadata
        .get(&target)
        .and_then(|metadata| metadata.name.as_deref())
        .filter(|name| !name.trim().is_empty())
}

fn terminal_metadata_by_target() -> Result<HashMap<String, state::TerminalMetadataRow>> {
    Ok(state::load_terminal_metadata()?
        .into_iter()
        .map(|metadata| (metadata.target.clone(), metadata))
        .collect())
}

fn used_terminal_display_names(
    metadata: &HashMap<String, state::TerminalMetadataRow>,
) -> Result<HashSet<String>> {
    Ok(AgentState::load()?
        .records
        .into_iter()
        .filter(|record| process_state(record.pid) == "running")
        .filter_map(|record| terminal_display_name(&record, metadata).map(normalize_display_name))
        .collect())
}

fn terminal_target_key(record: &AgentRecord) -> Option<String> {
    match terminal_target(record)? {
        TerminalTarget::Tmux { session } => Some(format!("{session}:0.0")),
        TerminalTarget::Zellij { session, pane } => Some(format!("zellij:{session}:{pane}")),
    }
}

fn choose_agent_display_name(id: &str, used: &HashSet<String>) -> String {
    let start = stable_name_offset(id);
    for round in 0..100 {
        for offset in 0..AGENT_DISPLAY_NAMES.len() {
            let name = AGENT_DISPLAY_NAMES[(start + offset) % AGENT_DISPLAY_NAMES.len()];
            let candidate = if round == 0 {
                name.to_string()
            } else {
                format!("{name} {}", round + 1)
            };
            if !used.contains(&normalize_display_name(&candidate)) {
                return candidate;
            }
        }
    }
    format!("Agent {}", short_agent_id(id))
}

fn stable_name_offset(value: &str) -> usize {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    usize::try_from(hash % AGENT_DISPLAY_NAMES.len() as u64).unwrap_or(0)
}

fn normalize_display_name(value: impl AsRef<str>) -> String {
    value.as_ref().trim().to_ascii_lowercase()
}

fn short_agent_id(id: &str) -> String {
    id.chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn process_state(pid: u32) -> &'static str {
    if PathBuf::from(format!("/proc/{pid}")).exists() {
        "running"
    } else {
        "exited"
    }
}

fn log_path(id: &str, stream: &str) -> Result<PathBuf> {
    Ok(state_dir()?.join("logs").join(format!("{id}.{stream}.log")))
}

fn log_file(path: &PathBuf) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open agent log {}", path.display()))
}

fn sanitize_id(value: &str) -> String {
    let id: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    id.trim_matches('-').to_string()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
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
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            Some(_) | None => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn unix_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRecord {
    pub(crate) id: String,
    pub(crate) track: String,
    pub(crate) pid: u32,
    pub(crate) started_at: u64,
    pub(crate) command: Vec<String>,
    pub(crate) cwd: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalAgentContext {
    pub id: String,
    pub track: String,
    pub session: String,
    pub pane: String,
    pub target: String,
    pub started_at: u64,
    pub command: String,
}

struct AgentState {
    records: Vec<AgentRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapshotScope {
    All,
    RunningOnly,
}

impl AgentState {
    fn load() -> Result<Self> {
        let records = state::load_agents(&registry_path()?)?
            .into_iter()
            .map(|row| AgentRecord {
                id: row.id,
                track: row.track,
                pid: row.pid,
                started_at: row.started_at,
                command: row.command,
                cwd: row.cwd,
            })
            .collect();
        Ok(Self { records })
    }
}

#[cfg(test)]
fn parse_record(line: &str) -> Result<AgentRecord> {
    let fields = line.split('\t').collect::<Vec<_>>();
    if fields.len() != 5 {
        bail!("invalid agent registry line: {line}");
    }
    Ok(AgentRecord {
        id: unescape_field(fields[0]),
        track: unescape_field(fields[1]),
        pid: fields[2]
            .parse()
            .with_context(|| format!("invalid agent pid: {}", fields[2]))?,
        started_at: fields[3]
            .parse()
            .with_context(|| format!("invalid agent start time: {}", fields[3]))?,
        command: unescape_field(fields[4])
            .split('\u{1f}')
            .map(ToString::to_string)
            .collect(),
        cwd: None,
    })
}

#[cfg(test)]
fn serialize_record(record: &AgentRecord) -> String {
    [
        escape_field(&record.id),
        escape_field(&record.track),
        record.pid.to_string(),
        record.started_at.to_string(),
        escape_field(&record.command.join("\u{1f}")),
    ]
    .join("\t")
}

#[cfg(test)]
fn escape_field(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\t', "\\t")
}

#[cfg(test)]
fn unescape_field(value: &str) -> String {
    value.replace("\\t", "\t").replace("\\\\", "\\")
}

pub(crate) fn registry_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("agents.tsv"))
}

fn state_dir() -> Result<PathBuf> {
    if let Ok(path) = env::var("QCOLD_STATE_DIR") {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    let home = env::var("HOME").context("HOME is required when QCOLD_STATE_DIR is unset")?;
    Ok(PathBuf::from(home).join(".local/state/qcold"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use tempfile::tempdir;

    fn git_ok(cwd: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .current_dir(cwd)
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git command failed in {}: {:?}",
            cwd.display(),
            args
        );
    }

    fn seed_git_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        git_ok(path, &["init"]);
        git_ok(path, &["config", "user.name", "tester"]);
        git_ok(path, &["config", "user.email", "tester@example.com"]);
        fs::write(path.join("README.md"), "seed\n").unwrap();
        git_ok(path, &["add", "README.md"]);
        git_ok(path, &["commit", "-m", "seed"]);
    }

    #[test]
    fn records_round_trip() {
        let record = AgentRecord {
            id: "agent-1".to_string(),
            track: "track".to_string(),
            pid: 123,
            started_at: 456,
            command: vec!["sh".to_string(), "-c".to_string(), "echo hi".to_string()],
            cwd: None,
        };
        assert_eq!(parse_record(&serialize_record(&record)).unwrap(), record);
    }

    #[test]
    fn codex_wrappers_use_agent_worktree_launch_cwd() {
        let temp = tempdir().unwrap();
        assert!(command_contains_codex_agent("cc1 \"inspect submodules\""));
        assert!(command_contains_codex_agent(
            "/home/qqrm/.local/bin/cc2 \"fix context reset\""
        ));
        assert!(command_contains_codex_agent("codex3 exec \"audit\""));
        assert!(!command_contains_codex_agent("printf ok"));
        assert!(should_open_managed_worktree(true, temp.path()));
        assert!(!should_open_managed_worktree(false, temp.path()));
    }

    #[test]
    fn agent_worktree_paths_are_separate_from_task_inventory() {
        let temp = tempdir().unwrap();
        let primary = temp.path().join("repo");
        fs::create_dir_all(&primary).unwrap();
        assert_eq!(
            agent_worktree_path(&primary, "agent-c1-123")
                .unwrap()
                .strip_prefix(temp.path())
                .unwrap(),
            Path::new("WT/repo/agents/agent-c1-123")
        );
    }

    #[test]
    fn agent_worktree_creation_does_not_create_task_env() {
        let temp = tempdir().unwrap();
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);

        let context = open_agent_worktree("c1-123", "c1", 123, &primary).unwrap();
        assert_eq!(context.qcold_repo_root.as_deref(), Some(primary.as_path()));
        assert_eq!(
            context.qcold_agent_worktree.as_deref(),
            Some(context.cwd.as_path())
        );
        assert!(context
            .cwd
            .strip_prefix(temp.path().join("WT/repo/agents"))
            .is_ok());
        assert!(!context.cwd.join(".task/task.env").exists());
    }

    #[test]
    fn agent_worktree_initializes_local_file_submodules() {
        let temp = tempdir().unwrap();
        let submodule = temp.path().join("json11-src");
        seed_git_repo(&submodule);

        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let submodule_arg = submodule.display().to_string();
        git_ok(
            &primary,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                &submodule_arg,
                "json11",
            ],
        );
        git_ok(&primary, &["commit", "-m", "add json11 submodule"]);

        let context = open_agent_worktree("c1-submodule", "c1", 456, &primary).unwrap();
        assert!(context.cwd.join("json11/README.md").is_file());
    }

    #[test]
    fn terminal_env_prefix_exports_agent_worktree() {
        let prefix = terminal_qcold_env_prefix(
            Some(Path::new("/workspace/primary")),
            Some(Path::new("/workspace/WT/repo/agents/c1")),
        );
        assert!(prefix.contains("export QCOLD_REPO_ROOT='/workspace/primary';"));
        assert!(prefix.contains("export QCOLD_AGENT_WORKTREE='/workspace/WT/repo/agents/c1';"));
    }

    #[test]
    fn agent_display_name_uses_unused_pool_name() {
        let mut used = HashSet::new();
        for name in AGENT_DISPLAY_NAMES {
            used.insert(normalize_display_name(name));
        }

        let name = choose_agent_display_name("c1-1234", &used);
        assert!(name.ends_with(" 2"));
        assert!(!used.contains(&normalize_display_name(&name)));
    }

    #[test]
    fn snapshot_line_includes_terminal_display_name() {
        let record = AgentRecord {
            id: "c1-1234".to_string(),
            track: "c1".to_string(),
            pid: std::process::id(),
            started_at: 456,
            command: vec![
                "tmux".to_string(),
                "new-session".to_string(),
                "-s".to_string(),
                "qcold-c1-1234".to_string(),
                "c1 \"inspect\"".to_string(),
            ],
            cwd: None,
        };
        let mut metadata = HashMap::new();
        metadata.insert(
            "qcold-c1-1234:0.0".to_string(),
            state::TerminalMetadataRow {
                target: "qcold-c1-1234:0.0".to_string(),
                name: Some("Socrates".to_string()),
                scope: None,
                updated_at: 123,
            },
        );

        assert!(render_record(&record, &metadata).contains("\tname=Socrates\t"));
    }

    #[test]
    fn terminal_attach_selector_matches_id_target_session_and_name() {
        let record = AgentRecord {
            id: "c1-1234".to_string(),
            track: "c1".to_string(),
            pid: std::process::id(),
            started_at: 456,
            command: vec![
                "tmux".to_string(),
                "new-session".to_string(),
                "-s".to_string(),
                "qcold-c1-1234".to_string(),
                "c1 \"inspect\"".to_string(),
            ],
            cwd: None,
        };
        let records = vec![record];
        let mut metadata = HashMap::new();
        metadata.insert(
            "qcold-c1-1234:0.0".to_string(),
            state::TerminalMetadataRow {
                target: "qcold-c1-1234:0.0".to_string(),
                name: Some("Socrates".to_string()),
                scope: None,
                updated_at: 123,
            },
        );

        for selector in ["c1-1234", "qcold-c1-1234", "qcold-c1-1234:0.0", "socrates"] {
            let matches = terminal_attach_matches(&records, &metadata, selector);
            assert_eq!(matches.len(), 1, "selector={selector}");
            assert_eq!(matches[0].id, "c1-1234");
        }
    }

    #[test]
    fn terminal_attach_selector_ignores_non_terminal_agents() {
        let records = vec![AgentRecord {
            id: "c1-plain".to_string(),
            track: "c1".to_string(),
            pid: std::process::id(),
            started_at: 456,
            command: vec!["c1".to_string(), "inspect".to_string()],
            cwd: None,
        }];

        let matches = terminal_attach_matches(&records, &HashMap::new(), "c1-plain");

        assert!(matches.is_empty());
    }

    #[test]
    fn running_snapshot_omits_exited_agent_records() {
        let records = vec![
            AgentRecord {
                id: "active-agent".to_string(),
                track: "unit".to_string(),
                pid: std::process::id(),
                started_at: 100,
                command: vec!["sleep".to_string(), "10".to_string()],
                cwd: None,
            },
            AgentRecord {
                id: "exited-agent".to_string(),
                track: "unit".to_string(),
                pid: u32::MAX,
                started_at: 101,
                command: vec!["printf".to_string(), "done".to_string()],
                cwd: None,
            },
        ];
        let metadata = HashMap::new();

        let snapshot =
            render_snapshot_with_metadata(&records, SnapshotScope::RunningOnly, &metadata);

        assert!(snapshot.starts_with("agents\tcount=1\n"));
        assert!(snapshot.contains("agent\tactive-agent\t"));
        assert!(!snapshot.contains("exited-agent"));
    }

    #[test]
    fn start_shell_agent_records_process() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path());
        let record = start_agent(
            None,
            "unit",
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "sleep 1".to_string(),
            ],
            Some(temp.path()),
        )
        .unwrap();
        assert!(record.id.starts_with("unit-"));
        let snapshot = snapshot().unwrap();
        assert!(snapshot.contains("agent\tunit-"));
    }
}
