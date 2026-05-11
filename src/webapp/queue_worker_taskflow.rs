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
    let output = Command::new(env::current_exe().context("failed to locate Q-COLD executable")?)
        .current_dir(&repo_root)
        .env("QCOLD_REPO_ROOT", &repo_root)
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
    let expected = format!("TASK_NAME={task_slug}");
    if !content.lines().any(|line| line == expected) {
        bail!("{} does not describe task {task_slug}", env_path.display());
    }
    Ok(())
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
    let description = existing.as_ref().map_or_else(
        || format!("Open managed task-flow work for {title}."),
        |record| record.description.clone(),
    );
    let metadata = serde_json::json!({
        "task_slug": item.slug,
        "kind": "managed-task-flow",
        "opened_by": "web-queue"
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
