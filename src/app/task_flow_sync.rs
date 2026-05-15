fn sync_task_flow_records(records: &[state::TaskRecordRow]) -> Result<usize> {
    let mut worktrees = std::collections::BTreeMap::new();
    for root in task_flow_scan_roots(records) {
        for worktree in task_flow_worktrees_for_repo(&root)? {
            worktrees.entry(worktree).or_insert(None);
        }
    }
    let mut synced = 0;
    for record in records {
        if record.source != "task-flow" {
            continue;
        }
        let Some(worktree) = task_flow_worktree_for_record(record) else {
            continue;
        };
        worktrees.insert(worktree, Some(record.status.clone()));
    }
    for (worktree, status_override) in worktrees {
        if sync_task_flow_record_for_worktree(&worktree, status_override.as_deref())? {
            synced += 1;
        }
    }
    Ok(synced)
}

#[allow(clippy::too_many_lines, reason = "existing task-flow sync debt")]
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
        let mut record = state::new_task_record(
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
        );
        record.sequence = env
            .get("TASK_SEQUENCE")
            .or_else(|| env.get("TASK_EXECUTION_ANCHOR"))
            .and_then(|value| value.parse::<u64>().ok());
        record
    });
    let original_status = record.status.clone();
    let original_repo_root = record.repo_root.clone();
    let original_cwd = record.cwd.clone();
    let original_metadata_json = record.metadata_json.clone();
    let start = env
        .get("STARTED_AT")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(record.created_at);
    let task_is_live = env
        .get("STATUS")
        .is_none_or(|status| matches!(status.as_str(), "open" | "paused" | "failed-closeout"));
    let finish = if task_is_live {
        unix_now()
    } else {
        env.get("UPDATED_AT")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_else(unix_now)
    };
    let task_slug = record_id.strip_prefix("task/").map(str::to_string);
    let explicit_rollout_paths = explicit_codex_rollout_paths_from_env(&env);
    let explicit_thread_id = env
        .get("CODEX_THREAD_ID")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let telemetry = codex_task_telemetry_for_worktree(
        worktree,
        task_slug.as_deref(),
        start,
        finish,
        &explicit_rollout_paths,
        explicit_thread_id,
    )?;

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
    if let Some(task_slug) = task_slug.as_deref() {
        metadata.insert("task_slug".to_string(), Value::String(task_slug.to_string()));
    }
    metadata.insert(
        "task_worktree".to_string(),
        Value::String(worktree.display().to_string()),
    );
    metadata.insert("task_started_at".to_string(), Value::from(start));
    if task_is_live {
        metadata.remove("task_finished_at");
    } else {
        metadata.insert("task_finished_at".to_string(), Value::from(finish));
    }
    if let Some(telemetry) = telemetry {
        if let Some(session_path) = telemetry.session_paths.first() {
            metadata.insert(
                "session_path".to_string(),
                Value::String(session_path.clone()),
            );
            metadata.insert(
                "session_paths".to_string(),
                Value::Array(
                    telemetry
                        .session_paths
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        metadata.insert(
            "session_ids".to_string(),
            Value::Array(
                telemetry
                    .session_ids
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        metadata.insert("token_usage".to_string(), telemetry.usage.as_json());
        metadata.insert(
            "token_efficiency".to_string(),
            telemetry.efficiency_json(unix_now(), start, finish),
        );
    } else if !metadata_session_path_matches_worktree(&metadata, worktree) {
        remove_codex_task_metadata(&mut metadata);
    }

    record.cwd = Some(worktree.display().to_string());
    if let Some(primary) = env.get("PRIMARY_REPO_PATH") {
        record.repo_root = Some(primary.clone());
    }
    if let Some(status) = status_override {
        record.status = status.to_string();
    } else if let Some(status) = env.get("STATUS") {
        record.status.clone_from(status);
    }
    let metadata = Value::Object(metadata);
    if original_status == record.status
        && original_repo_root == record.repo_root
        && original_cwd == record.cwd
        && original_metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .is_some_and(|existing| task_flow_metadata_equivalent(&existing, &metadata))
    {
        return Ok(false);
    }
    record.metadata_json = Some(metadata.to_string());
    record.updated_at = unix_now();
    state::upsert_task_record(&record)?;
    Ok(true)
}

fn explicit_codex_rollout_paths_from_env(
    env: &std::collections::BTreeMap<String, String>,
) -> Vec<PathBuf> {
    env.get("CODEX_ROLLOUT_PATH")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .collect()
}

fn task_flow_metadata_equivalent(left: &Value, right: &Value) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    remove_task_flow_capture_timestamp(&mut left);
    remove_task_flow_capture_timestamp(&mut right);
    left == right
}

fn remove_task_flow_capture_timestamp(value: &mut Value) {
    if let Some(efficiency) = value
        .get_mut("token_efficiency")
        .and_then(Value::as_object_mut)
    {
        efficiency.remove("captured_at");
    }
}

fn remove_codex_task_metadata(metadata: &mut serde_json::Map<String, Value>) {
    metadata.remove("session_path");
    metadata.remove("session_paths");
    metadata.remove("session_ids");
    metadata.remove("token_usage");
    metadata.remove("token_efficiency");
}

fn metadata_session_path_matches_worktree(
    metadata: &serde_json::Map<String, Value>,
    worktree: &Path,
) -> bool {
    metadata
        .get("session_path")
        .and_then(Value::as_str)
        .and_then(|path| fs::read_to_string(path).ok())
        .is_some_and(|content| {
            codex_session_match_for_worktree(&content, &worktree.display().to_string()).is_some()
        })
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

fn task_flow_scan_roots(records: &[state::TaskRecordRow]) -> Vec<PathBuf> {
    let mut roots = std::collections::BTreeSet::new();
    if let Ok(root) = repository::active_root() {
        roots.insert(root);
    }
    for record in records {
        if let Some(root) = record.repo_root.as_deref() {
            roots.insert(PathBuf::from(root));
        }
    }
    roots.into_iter().collect()
}

fn task_flow_worktrees_for_repo(repo_root: &Path) -> Result<Vec<PathBuf>> {
    let managed_root = managed_root_for(repo_root);
    let entries = match fs::read_dir(&managed_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read {}", managed_root.display()));
        }
    };
    let mut worktrees = Vec::new();
    for entry in entries {
        let entry = entry
            .with_context(|| format!("failed to read entry in {}", managed_root.display()))?;
        let worktree = entry.path();
        if worktree.join(".task/task.env").is_file() {
            worktrees.push(worktree);
        }
    }
    worktrees.sort();
    Ok(worktrees)
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
    if !(lower.contains("c1")
        || lower.contains("cc1")
        || lower.contains("c2")
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
        if name == "c1" || name == "cc1" {
            return Some("1".to_string());
        }
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
