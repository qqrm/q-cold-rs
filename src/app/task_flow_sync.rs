const TERMINAL_BUNDLE_SCAN_LIMIT: usize = 250;

#[derive(Clone, Debug)]
struct TerminalTaskBundle {
    status: String,
    duration_seconds: Option<u64>,
    bundle_path: PathBuf,
}

fn sync_task_flow_records(records: &[state::TaskRecordRow]) -> Result<usize> {
    let mut worktrees = std::collections::BTreeMap::new();
    let roots = task_flow_scan_roots(records);
    for root in &roots {
        for worktree in task_flow_worktrees_for_repo(root)? {
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
        let status_override = task_record_terminal_status(&record.status).map(ToString::to_string);
        worktrees.insert(worktree, status_override);
    }
    for (worktree, status_override) in worktrees {
        if sync_task_flow_record_for_worktree(&worktree, status_override.as_deref())? {
            synced += 1;
        }
    }
    let terminal_bundles = terminal_task_bundles_for_records(records, &roots)?;
    for record in records {
        if !task_flow_record_needs_terminal_sync(record) {
            continue;
        }
        if let Some(bundle) = terminal_bundles.get(&record.id) {
            if sync_task_flow_record_from_terminal_bundle(record, bundle)? {
                synced += 1;
            }
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
    let mut metadata = record
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let suppress_live_queue_telemetry =
        live_web_queue_task(&metadata, task_is_live, task_slug.as_deref());
    if suppress_live_queue_telemetry {
        metadata.insert(
            "opened_by".to_string(),
            Value::String("web-queue".to_string()),
        );
    }
    let explicit_rollout_paths = explicit_codex_rollout_paths_from_env(&env);
    let explicit_thread_id = env
        .get("CODEX_THREAD_ID")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let telemetry = if suppress_live_queue_telemetry {
        None
    } else {
        codex_task_telemetry_for_worktree(
            worktree,
            task_slug.as_deref(),
            start,
            finish,
            &explicit_rollout_paths,
            explicit_thread_id,
        )?
    };
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
    } else if suppress_live_queue_telemetry
        || !metadata_session_path_matches_worktree(&metadata, worktree)
    {
        remove_codex_task_metadata(&mut metadata);
    }

    record.cwd = Some(worktree.display().to_string());
    if let Some(primary) = env.get("PRIMARY_REPO_PATH") {
        record.repo_root = Some(primary.clone());
    }
    if let Some(status) = status_override {
        record.status = status.to_string();
    } else if let Some(status) = env.get("STATUS") {
        record.status = task_record_status_from_task_flow_status(status);
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

fn task_record_terminal_status(status: &str) -> Option<&str> {
    status.starts_with("closed:").then_some(status)
}

fn task_record_status_from_task_flow_status(status: &str) -> String {
    match status {
        "success" | "blocked" | "failed" => format!("closed:{status}"),
        _ => status.to_string(),
    }
}

fn task_flow_record_needs_terminal_sync(record: &state::TaskRecordRow) -> bool {
    record.source == "task-flow" && task_record_terminal_status(&record.status).is_none()
}

fn live_web_queue_task(
    metadata: &serde_json::Map<String, Value>,
    task_is_live: bool,
    task_slug: Option<&str>,
) -> bool {
    if !task_is_live {
        return false;
    }
    if metadata
        .get("opened_by")
        .and_then(Value::as_str)
        .is_some_and(|value| value == "web-queue")
    {
        return true;
    }
    task_slug.is_some_and(live_web_queue_item_slug)
}

fn live_web_queue_item_slug(task_slug: &str) -> bool {
    state::load_web_queue_items().is_ok_and(|items| {
        items
            .iter()
            .any(|item| item.slug == task_slug && !item.status.is_success())
    })
}

fn terminal_task_bundles_for_records(
    records: &[state::TaskRecordRow],
    roots: &[PathBuf],
) -> Result<std::collections::BTreeMap<String, TerminalTaskBundle>> {
    let mut wanted_by_root = std::collections::BTreeMap::<PathBuf, std::collections::BTreeSet<String>>::new();
    for record in records {
        if !task_flow_record_needs_terminal_sync(record) {
            continue;
        }
        let Some(root) = record.repo_root.as_deref().map(PathBuf::from) else {
            continue;
        };
        wanted_by_root.entry(root).or_default().insert(record.id.clone());
    }
    for root in roots {
        wanted_by_root.entry(root.clone()).or_default();
    }

    let mut found = std::collections::BTreeMap::new();
    for (root, wanted) in wanted_by_root {
        if wanted.is_empty() {
            continue;
        }
        found.extend(terminal_task_bundles_for_root(&root, &wanted)?);
    }
    Ok(found)
}

fn terminal_task_bundles_for_root(
    root: &Path,
    wanted: &std::collections::BTreeSet<String>,
) -> Result<std::collections::BTreeMap<String, TerminalTaskBundle>> {
    let mut found = std::collections::BTreeMap::new();
    for bundle in terminal_bundle_candidates(root)? {
        if found.len() == wanted.len() {
            break;
        }
        let Some(receipt) = unzip_env_entry(&bundle, "metadata/terminal-receipt.env") else {
            continue;
        };
        let Some(task_id) = receipt.get("TASK_ID").cloned() else {
            continue;
        };
        if !wanted.contains(&task_id) || found.contains_key(&task_id) {
            continue;
        }
        let Some(status) = receipt
            .get("OUTCOME")
            .map(String::as_str)
            .map(task_record_status_from_task_flow_status)
            .filter(|status| task_record_terminal_status(status).is_some())
        else {
            continue;
        };
        let bundle_env = unzip_env_entry(&bundle, "metadata/bundle.env").unwrap_or_default();
        let duration_seconds = bundle_env
            .get("TASK_DURATION_SECONDS")
            .and_then(|value| value.parse::<u64>().ok());
        found.insert(
            task_id,
            TerminalTaskBundle {
                status,
                duration_seconds,
                bundle_path: bundle,
            },
        );
    }
    Ok(found)
}

fn terminal_bundle_candidates(root: &Path) -> Result<Vec<PathBuf>> {
    let bundles_dir = root.join("bundles");
    let entries = match fs::read_dir(&bundles_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read {}", bundles_dir.display()));
        }
    };
    let mut bundles = Vec::new();
    for entry in entries {
        let entry = entry
            .with_context(|| format!("failed to read entry in {}", bundles_dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("zip") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        bundles.push((modified, path));
    }
    bundles.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    Ok(bundles
        .into_iter()
        .map(|(_, path)| path)
        .take(TERMINAL_BUNDLE_SCAN_LIMIT)
        .collect())
}

fn unzip_env_entry(
    bundle: &Path,
    entry: &str,
) -> Option<std::collections::BTreeMap<String, String>> {
    let output = Command::new("unzip")
        .arg("-p")
        .arg(bundle)
        .arg(entry)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let content = String::from_utf8_lossy(&output.stdout);
    Some(parse_task_env_content(&content))
}

fn sync_task_flow_record_from_terminal_bundle(
    record: &state::TaskRecordRow,
    bundle: &TerminalTaskBundle,
) -> Result<bool> {
    let mut updated = record.clone();
    let original_status = updated.status.clone();
    let original_metadata_json = updated.metadata_json.clone();
    let mut metadata = updated
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if let Some(task_slug) = updated.id.strip_prefix("task/") {
        metadata.insert("task_slug".to_string(), Value::String(task_slug.to_string()));
    }
    metadata.insert(
        "kind".to_string(),
        Value::String("managed-task-flow".to_string()),
    );
    metadata.insert(
        "task_terminal_bundle".to_string(),
        Value::String(bundle.bundle_path.display().to_string()),
    );
    if let Some(duration) = bundle.duration_seconds {
        metadata.insert("task_duration_seconds".to_string(), Value::from(duration));
    }
    let start = metadata
        .get("task_started_at")
        .and_then(Value::as_u64)
        .unwrap_or(updated.created_at);
    metadata.insert("task_started_at".to_string(), Value::from(start));
    let finish = bundle
        .duration_seconds
        .map_or(updated.updated_at.max(updated.created_at), |duration| {
            start.saturating_add(duration)
        });
    metadata.insert("task_finished_at".to_string(), Value::from(finish));
    updated.status.clone_from(&bundle.status);
    updated.updated_at = finish;
    let metadata = Value::Object(metadata);
    if original_status == updated.status
        && original_metadata_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .is_some_and(|existing| task_flow_metadata_equivalent(&existing, &metadata))
    {
        return Ok(false);
    }
    updated.metadata_json = Some(metadata.to_string());
    state::upsert_task_record(&updated)?;
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
    Ok(parse_task_env_content(&content))
}

fn parse_task_env_content(content: &str) -> std::collections::BTreeMap<String, String> {
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
    entries
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
