const QUEUE_TASK_OPEN_OUTPUT_LIMIT: usize = 1200;

struct QueueManagedTask {
    worktree: PathBuf,
}

fn ensure_queue_managed_task(item: &state::QueueItemRow) -> Result<QueueManagedTask> {
    if let Some(worktree) = existing_queue_task_worktree(&item.slug)? {
        return Ok(QueueManagedTask { worktree });
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
    Ok(QueueManagedTask { worktree })
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

fn remember_queue_task_agent(item: &state::QueueItemRow, agent_id: &str) -> Result<()> {
    let task_id = format!("task/{}", item.slug);
    let Some(mut record) = state::get_task_record(&task_id)? else {
        return Ok(());
    };
    record.agent_id = Some(agent_id.to_string());
    state::upsert_task_record(&record)?;
    Ok(())
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
