struct NamedCodexResumeLaunch {
    command: String,
    cwd: PathBuf,
}

fn named_codex_resume_launch(
    track: &str,
    command: &str,
    requested_cwd: Option<&Path>,
    requested_name: Option<&str>,
) -> Result<Option<NamedCodexResumeLaunch>> {
    if requested_cwd.is_some() {
        return Ok(None);
    }
    let Some(requested_name) = requested_name.filter(|name| !name.trim().is_empty()) else {
        return Ok(None);
    };
    let Some(primary_root) = current_launch_primary_root()? else {
        return Ok(None);
    };
    named_codex_resume_launch_for_primary(track, command, requested_name, &primary_root)
}

fn named_codex_resume_launch_for_primary(
    track: &str,
    command: &str,
    requested_name: &str,
    primary_root: &Path,
) -> Result<Option<NamedCodexResumeLaunch>> {
    let Some(command_token) = plain_codex_chat_command(command) else {
        return Ok(None);
    };
    let Some(account) = codex_account_from_command(command) else {
        return Ok(None);
    };
    let _ = crate::sync_codex_task_records();
    let metadata = terminal_metadata_by_target().unwrap_or_default();
    let tasks = state::load_task_records(None, 1000)?;
    let mut records = AgentState::load()?.records;
    records.sort_by_key(|record| std::cmp::Reverse(record.started_at));
    for record in records {
        let Some(cwd) = named_resume_candidate_cwd(
            &record,
            track,
            &account,
            requested_name,
            primary_root,
            &metadata,
        )?
        else {
            continue;
        };
        if let Some(session_id) = task_resume_session_for_agent(&tasks, &record.id) {
            return Ok(Some(NamedCodexResumeLaunch {
                command: format!(
                    "{} resume {}",
                    shell_quote(&command_token),
                    shell_quote(&session_id)
                ),
                cwd,
            }));
        }
    }
    Ok(None)
}

fn current_launch_primary_root() -> Result<Option<PathBuf>> {
    let cwd = resolve_codex_launch_cwd()?;
    if let Some((_, primary_root)) = agent_worktree_primary_for_cwd(&cwd)? {
        return Ok(Some(primary_root));
    }
    Ok(git_root_for(&cwd).ok().map(|root| {
        root.canonicalize().unwrap_or_else(|_| root.clone())
    }))
}

fn plain_codex_chat_command(command: &str) -> Option<String> {
    let words = shell_words(command);
    words.iter().enumerate().find_map(|(index, word)| {
        if index + 1 != words.len() {
            return None;
        }
        let name = Path::new(word).file_name()?.to_str()?;
        is_codex_agent_command(name).then(|| word.clone())
    })
}

fn named_resume_candidate_cwd(
    record: &AgentRecord,
    track: &str,
    account: &str,
    requested_name: &str,
    primary_root: &Path,
    metadata: &HashMap<String, state::TerminalMetadataRow>,
) -> Result<Option<PathBuf>> {
    if record.track != track || process_state(record.pid) == "running" {
        return Ok(None);
    }
    let Some(name) = terminal_display_name(record, metadata) else {
        return Ok(None);
    };
    if normalize_display_name(name) != normalize_display_name(requested_name) {
        return Ok(None);
    }
    let previous_command = terminal_command_from_record(&record.command);
    if codex_account_from_command(&previous_command).as_deref() != Some(account) {
        return Ok(None);
    }
    let Some(cwd) = record.cwd.clone().filter(|cwd| cwd.is_dir()) else {
        return Ok(None);
    };
    let Some((_, candidate_primary)) = agent_worktree_primary_for_cwd(&cwd)? else {
        return Ok(None);
    };
    Ok((candidate_primary == primary_root).then_some(cwd))
}

fn task_resume_session_for_agent(records: &[state::TaskRecordRow], agent_id: &str) -> Option<String> {
    records
        .iter()
        .filter(|record| record.agent_id.as_deref() == Some(agent_id))
        .find_map(|record| codex_resume_session_id_from_metadata(record.metadata_json.as_deref()))
}

fn codex_resume_session_id_from_metadata(metadata_json: Option<&str>) -> Option<String> {
    let metadata = serde_json::from_str::<serde_json::Value>(metadata_json?).ok()?;
    metadata
        .get("codex_thread_id")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            metadata
                .get("session_path")
                .and_then(serde_json::Value::as_str)
                .and_then(codex_thread_id_from_session_path)
        })
}

fn codex_thread_id_from_session_path(path: &str) -> Option<String> {
    let stem = Path::new(path).file_stem()?.to_str()?;
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

fn reusable_codex_agent_context(
    track: &str,
    command: &str,
    requested_cwd: Option<&Path>,
    base_cwd: &Path,
) -> Result<Option<LaunchContext>> {
    if requested_cwd.is_some() {
        return Ok(None);
    }
    let Some(primary_root) = git_root_for(base_cwd).ok() else {
        return Ok(None);
    };
    let include_running = command_is_codex_resume(command);
    if !include_running && !command_is_interactive_codex_launch(command) {
        return Ok(None);
    }
    let Some(cwd) = latest_agent_cwd_for_launch(track, command, &primary_root, include_running)?
    else {
        return Ok(None);
    };
    let qcold_agent_worktree = git_root_for(&cwd).ok();
    Ok(Some(LaunchContext {
        cwd,
        qcold_repo_root: Some(primary_root),
        qcold_agent_worktree,
    }))
}

fn command_is_interactive_codex_launch(command: &str) -> bool {
    let words = shell_words(command);
    words.iter().enumerate().any(|(index, word)| {
        Path::new(word)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_codex_agent_command)
            && words.get(index + 1).is_none_or(|next| next != "exec")
    })
}

fn latest_agent_cwd_for_launch(
    track: &str,
    command: &str,
    primary_root: &Path,
    include_running: bool,
) -> Result<Option<PathBuf>> {
    let Some(account) = codex_account_from_command(command) else {
        return Ok(None);
    };
    let agents_dir = agent_worktrees_dir(primary_root)?;
    let mut records = AgentState::load()?.records;
    records.sort_by_key(|record| std::cmp::Reverse(record.started_at));
    Ok(records.into_iter().find_map(|record| {
        let cwd = record.cwd?;
        if record.track != track || !cwd.is_dir() || !cwd.starts_with(&agents_dir) {
            return None;
        }
        if !include_running && process_state(record.pid) == "running" {
            return None;
        }
        if !include_running && !git_worktree_matches_current(&cwd, primary_root) {
            return None;
        }
        (codex_account_from_command(&terminal_command_from_record(&record.command)).as_deref()
            == Some(account.as_str()))
        .then_some(cwd)
    }))
}

fn existing_agent_worktree_context(cwd: &Path) -> Result<Option<LaunchContext>> {
    let cwd = canonical_dir(cwd)?;
    let Some((agent_worktree, primary_root)) = agent_worktree_primary_for_cwd(&cwd)? else {
        return Ok(None);
    };
    Ok(Some(LaunchContext {
        cwd,
        qcold_repo_root: Some(primary_root),
        qcold_agent_worktree: Some(agent_worktree),
    }))
}

fn agent_worktree_primary_for_cwd(cwd: &Path) -> Result<Option<(PathBuf, PathBuf)>> {
    if let (Some(agent_worktree), Some(primary_root)) = (
        canonical_env_dir("QCOLD_AGENT_WORKTREE"),
        canonical_env_dir("QCOLD_REPO_ROOT"),
    ) {
        if cwd.starts_with(&agent_worktree) {
            return Ok(Some((agent_worktree, primary_root)));
        }
    }

    for ancestor in cwd.ancestors() {
        let Some(agents_dir) = ancestor.parent() else {
            continue;
        };
        if agents_dir.file_name().and_then(|name| name.to_str()) != Some("agents") {
            continue;
        }
        let Some(repo_wt_dir) = agents_dir.parent() else {
            continue;
        };
        let Some(wt_dir) = repo_wt_dir.parent() else {
            continue;
        };
        if wt_dir.file_name().and_then(|name| name.to_str()) != Some("WT") {
            continue;
        }
        let Some(primary_parent) = wt_dir.parent() else {
            continue;
        };
        let Some(repo_name) = repo_wt_dir.file_name() else {
            continue;
        };
        let primary = primary_parent.join(repo_name);
        if !primary.is_dir() {
            continue;
        }
        let primary = canonical_dir(&primary)?;
        let agent_worktree = canonical_dir(ancestor)?;
        return Ok(Some((agent_worktree, primary)));
    }
    Ok(None)
}

fn canonical_env_dir(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok())
        .filter(|path| path.is_dir())
}

fn git_worktree_matches_current(left: &Path, right: &Path) -> bool {
    if git_head(left)
        .zip(git_head(right))
        .is_none_or(|(left, right)| left != right)
    {
        return false;
    }
    match (git_branch(left), git_branch(right)) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn git_head(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git_branch(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn command_is_codex_resume(command: &str) -> bool {
    let words = shell_words(command);
    words.iter().enumerate().any(|(index, word)| {
        Path::new(word)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_codex_agent_command)
            && words
                .get(index + 1)
                .is_some_and(|next| next == "resume")
    })
}
