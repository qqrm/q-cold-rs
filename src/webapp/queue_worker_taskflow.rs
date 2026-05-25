const QUEUE_TASK_OPEN_OUTPUT_LIMIT: usize = 1200;

struct QueueManagedTask {
    worktree: PathBuf,
    remote_launcher: Option<String>,
    remote_worktree: Option<String>,
}

fn ensure_queue_managed_task(item: &state::QueueItemRow) -> Result<QueueManagedTask> {
    if let Some(launcher) = item.remote_launcher.as_deref() {
        return ensure_remote_queue_managed_task(item, launcher);
    }
    if let Some(worktree) = existing_queue_task_worktree(&item.slug)? {
        return Ok(QueueManagedTask {
            worktree,
            remote_launcher: None,
            remote_worktree: None,
        });
    }
    let repo_root = item
        .repo_root
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("queue item has no repository root")?;
    let repo_root = PathBuf::from(repo_root)
        .canonicalize()
        .with_context(|| format!("failed to resolve queue repository {repo_root}"))?;
    let output = Command::new(queue_qcold_executable()?)
        .current_dir(&repo_root)
        .env("QCOLD_REPO_ROOT", &repo_root)
        .env("QCOLD_TASKFLOW_PROMPT", &item.prompt)
        .env("QCOLD_TASK_PROMPT_SNIPPET", prompt::prompt_snippet(&item.prompt))
        .args(["task", "open", &item.slug])
        .output()
        .with_context(|| format!("failed to open managed task {}", item.slug))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "failed to open managed task {} in {}: {}\n{}",
            item.slug,
            repo_root.display(),
            output.status,
            compact_process_output(&stdout, &stderr)
        );
    }
    let worktree = parse_task_worktree_output(&stdout)
        .context("task open did not report TASK_WORKTREE")?;
    validate_queue_task_worktree(&worktree, &item.slug)?;
    remember_queue_task_worktree(item, &repo_root, &worktree)?;
    crate::sync_codex_task_records().ok();
    Ok(QueueManagedTask {
        worktree,
        remote_launcher: None,
        remote_worktree: None,
    })
}

fn ensure_remote_queue_managed_task(
    item: &state::QueueItemRow,
    launcher: &str,
) -> Result<QueueManagedTask> {
    let repo_root = queue_item_repo_root(item)?;
    if let Some(task) = existing_remote_queue_task(item, launcher, &repo_root)? {
        return Ok(task);
    }
    let output = Command::new(queue_qcold_executable()?)
        .current_dir(&repo_root)
        .env("QCOLD_REPO_ROOT", &repo_root)
        .env("QCOLD_TASKFLOW_PROMPT", &item.prompt)
        .env("QCOLD_TASK_PROMPT_SNIPPET", prompt::prompt_snippet(&item.prompt))
        .args(["task", "open-remote", "--via", launcher, &item.slug])
        .output()
        .with_context(|| {
            format!(
                "failed to open remote managed task {} through {launcher}",
                item.slug
            )
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "failed to open remote managed task {} through {launcher}: {}\n{}",
            item.slug,
            output.status,
            compact_process_output(&stdout, &stderr)
        );
    }
    let remote_worktree = parse_task_worktree_output(&stdout)
        .context("remote task open did not report TASK_WORKTREE")?
        .display()
        .to_string();
    remember_queue_remote_task(item, &repo_root, launcher, &remote_worktree)?;
    let _ = sync_remote_queue_task_records(item);
    Ok(QueueManagedTask {
        worktree: repo_root,
        remote_launcher: Some(launcher.to_string()),
        remote_worktree: Some(remote_worktree),
    })
}

fn queue_qcold_executable() -> Result<PathBuf> {
    let current = env::current_exe().context("failed to locate current Q-COLD executable")?;
    queue_qcold_executable_from(&current, env::var_os("PATH").as_deref())
}

fn queue_qcold_executable_from(current: &Path, path: Option<&std::ffi::OsStr>) -> Result<PathBuf> {
    if executable_file(current) {
        return Ok(current.to_path_buf());
    }
    if let Some(installed) = qcold_executable_from_path(path) {
        return Ok(installed);
    }
    bail!(
        "failed to locate runnable Q-COLD executable; current executable {} is unavailable and \
         qcold was not found on PATH",
        current.display()
    );
}

fn qcold_executable_from_path(path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let binary = format!("qcold{}", env::consts::EXE_SUFFIX);
    env::split_paths(path?).find_map(|directory| {
        let candidate = directory.join(&binary);
        executable_file(&candidate).then_some(candidate)
    })
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && has_execute_permission(&metadata)
}

#[cfg(unix)]
fn has_execute_permission(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn has_execute_permission(_metadata: &fs::Metadata) -> bool {
    true
}

fn existing_queue_task_worktree(task_slug: &str) -> Result<Option<PathBuf>> {
    let task_id = format!("task/{task_slug}");
    if let Some(record) = state::get_task_record(&task_id)? {
        if let Some(cwd) = record.cwd.as_deref().map(PathBuf::from) {
            if validate_queue_task_worktree(&cwd, task_slug).is_ok() {
                return Ok(Some(cwd));
            }
        }
        if let Some(repo_root) = record.repo_root.as_deref().map(PathBuf::from) {
            if let Some(worktree) = discover_queue_task_worktree(&repo_root, task_slug) {
                return Ok(Some(worktree));
            }
        }
    }
    Ok(None)
}

fn existing_remote_queue_task(
    item: &state::QueueItemRow,
    launcher: &str,
    repo_root: &Path,
) -> Result<Option<QueueManagedTask>> {
    let task_id = format!("task/{}", item.slug);
    let Some(record) = state::get_task_record(&task_id)? else {
        return Ok(None);
    };
    let metadata = task_record_metadata_object(&record);
    let remote_launcher = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("remote_launcher"))
        .and_then(Value::as_str);
    if remote_launcher != Some(launcher) {
        return Ok(None);
    }
    let remote_worktree = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("remote_cwd"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(record.cwd);
    let Some(remote_worktree) = remote_worktree.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    Ok(Some(QueueManagedTask {
        worktree: repo_root.to_path_buf(),
        remote_launcher: Some(launcher.to_string()),
        remote_worktree: Some(remote_worktree),
    }))
}

fn discover_queue_task_worktree(repo_root: &Path, task_slug: &str) -> Option<PathBuf> {
    let repo_root = repo_root.canonicalize().ok()?;
    let inventory_root = repo_root.parent()?.join("WT").join(repo_root.file_name()?);
    for entry in fs::read_dir(inventory_root).ok()? {
        let path = entry.ok()?.path();
        if validate_queue_task_worktree(&path, task_slug).is_ok() {
            return Some(path);
        }
    }
    None
}

fn parse_task_worktree_output(output: &str) -> Option<PathBuf> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("TASK_WORKTREE="))
        .map(PathBuf::from)
}

fn validate_queue_task_worktree(path: &Path, task_slug: &str) -> Result<()> {
    let env_path = path.join(".task/task.env");
    let content = fs::read_to_string(&env_path)
        .with_context(|| format!("missing task metadata at {}", env_path.display()))?;
    let matches_task = content.lines().any(|line| {
        let Some(raw) = line.strip_prefix("TASK_NAME=") else {
            return false;
        };
        shell_env_value(raw) == task_slug
    });
    if !matches_task {
        bail!("{} does not describe task {task_slug}", env_path.display());
    }
    Ok(())
}

fn shell_env_value(raw: &str) -> String {
    if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
        return raw[1..raw.len() - 1].replace("'\\''", "'");
    }
    raw.to_string()
}

fn remember_queue_task_worktree(
    item: &state::QueueItemRow,
    repo_root: &Path,
    worktree: &Path,
) -> Result<()> {
    let task_id = format!("task/{}", item.slug);
    let existing = state::get_task_record(&task_id)?;
    let title = existing
        .as_ref()
        .map_or_else(|| item.slug.clone(), |record| record.title.clone());
    let prompt_snippet = prompt::prompt_snippet(&item.prompt);
    let description = if prompt_snippet.is_empty() {
        format!("Open managed task-flow work for {title}.")
    } else {
        prompt_snippet.clone()
    };
    let metadata = serde_json::json!({
        "task_slug": item.slug,
        "queue_item_id": item.id,
        "queue_run_id": item.run_id,
        "kind": "managed-task-flow",
        "opened_by": "web-queue",
        "prompt_source": "web-queue-card",
        "operator_prompt": item.prompt,
        "operator_prompt_snippet": prompt_snippet,
    });
    let record = state::new_task_record(
        task_id,
        "task-flow".to_string(),
        title,
        description,
        "open".to_string(),
        Some(repo_root.display().to_string()),
        Some(worktree.display().to_string()),
        item.agent_id.clone(),
        Some(metadata.to_string()),
    );
    state::upsert_task_record(&record)?;
    Ok(())
}

fn remember_queue_remote_task(
    item: &state::QueueItemRow,
    repo_root: &Path,
    remote_launcher: &str,
    remote_worktree: &str,
) -> Result<()> {
    let task_id = format!("task/{}", item.slug);
    let existing = state::get_task_record(&task_id)?;
    let title = existing
        .as_ref()
        .map_or_else(|| item.slug.clone(), |record| record.title.clone());
    let prompt_snippet = prompt::prompt_snippet(&item.prompt);
    let description = if prompt_snippet.is_empty() {
        format!("Open remote managed task-flow work for {title}.")
    } else {
        prompt_snippet.clone()
    };
    let mut metadata = existing
        .as_ref()
        .and_then(task_record_metadata_object)
        .unwrap_or_default();
    metadata.insert("task_slug".to_string(), Value::String(item.slug.clone()));
    metadata.insert("queue_item_id".to_string(), Value::String(item.id.clone()));
    metadata.insert("queue_run_id".to_string(), Value::String(item.run_id.clone()));
    metadata.insert(
        "kind".to_string(),
        Value::String("managed-task-flow".to_string()),
    );
    metadata.insert(
        "opened_by".to_string(),
        Value::String("web-queue".to_string()),
    );
    metadata.insert(
        "prompt_source".to_string(),
        Value::String("web-queue-card".to_string()),
    );
    metadata.insert(
        "operator_prompt".to_string(),
        Value::String(item.prompt.clone()),
    );
    metadata.insert(
        "operator_prompt_snippet".to_string(),
        Value::String(prompt_snippet),
    );
    metadata.insert(
        "remote_launcher".to_string(),
        Value::String(remote_launcher.to_string()),
    );
    metadata.insert(
        "remote_cwd".to_string(),
        Value::String(remote_worktree.to_string()),
    );
    let status = existing
        .as_ref()
        .map_or_else(|| "open".to_string(), |record| record.status.clone());
    let agent_id = item
        .agent_id
        .clone()
        .or_else(|| existing.as_ref().and_then(|record| record.agent_id.clone()));
    let sequence = existing.as_ref().and_then(|record| record.sequence);
    let mut record = state::new_task_record(
        task_id,
        "task-flow".to_string(),
        title,
        description,
        status,
        Some(repo_root.display().to_string()),
        Some(remote_worktree.to_string()),
        agent_id,
        Some(Value::Object(metadata).to_string()),
    );
    if let Some(sequence) = sequence {
        record.sequence = Some(sequence);
    }
    state::upsert_task_record(&record)?;
    Ok(())
}

fn queue_item_repo_root(item: &state::QueueItemRow) -> Result<PathBuf> {
    let repo_root = item
        .repo_root
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("queue item has no repository root")?;
    PathBuf::from(repo_root)
        .canonicalize()
        .with_context(|| format!("failed to resolve queue repository {repo_root}"))
}

fn task_record_metadata_object(
    record: &state::TaskRecordRow,
) -> Option<serde_json::Map<String, Value>> {
    record
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
}

fn remember_queue_task_agent(item: &state::QueueItemRow, agent_id: &str) -> Result<()> {
    let task_id = format!("task/{}", item.slug);
    let Some(mut record) = state::get_task_record(&task_id)? else {
        return Ok(());
    };
    record.agent_id = Some(agent_id.to_string());
    state::upsert_task_record(&record)?;
    Ok(())
}

fn sync_remote_queue_task_records(item: &state::QueueItemRow) -> Result<()> {
    let Some(launcher) = item.remote_launcher.as_deref() else {
        return Ok(());
    };
    if !remote_queue_sync_due(item, launcher) {
        return Ok(());
    }
    let repo_root = queue_item_repo_root(item)?;
    let repo_root_arg = repo_root.display().to_string();
    let output = Command::new(queue_qcold_executable()?)
        .current_dir(&repo_root)
        .env("QCOLD_REPO_ROOT", &repo_root)
        .args([
            "task-record",
            "sync-remote",
            "--via",
            launcher,
            "--local-repo-root",
            &repo_root_arg,
        ])
        .output()
        .with_context(|| format!("failed to sync remote task records through {launcher}"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "remote task-record sync through {launcher} failed with {}: {}",
            output.status,
            compact_process_output(&stdout, &stderr)
        );
    }
    Ok(())
}

fn remote_queue_sync_due(item: &state::QueueItemRow, launcher: &str) -> bool {
    const REMOTE_QUEUE_SYNC_INTERVAL_SECS: u64 = 15;
    let repo = item.repo_root.as_deref().unwrap_or("");
    let key = format!("{launcher}\t{repo}");
    let now = unix_now();
    let sync_times = WEB_QUEUE_REMOTE_SYNC_AT.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut sync_times) = sync_times.lock() else {
        return true;
    };
    if sync_times
        .get(&key)
        .is_some_and(|last| now.saturating_sub(*last) < REMOTE_QUEUE_SYNC_INTERVAL_SECS)
    {
        return false;
    }
    sync_times.insert(key, now);
    true
}

fn compact_process_output(stdout: &str, stderr: &str) -> String {
    let mut output = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if output.len() > QUEUE_TASK_OPEN_OUTPUT_LIMIT {
        output.truncate(QUEUE_TASK_OPEN_OUTPUT_LIMIT);
        output.push_str("...");
    }
    output
}
