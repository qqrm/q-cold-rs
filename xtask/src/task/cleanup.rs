fn clear_terminal_task_worktrees(repo: &Path) -> Result<TaskWorktreeCleanup> {
    let mut removed = 0;
    for task in open_tasks(repo)? {
        if !task.status.starts_with("closed:") {
            continue;
        }
        clear_finished_task_worktree(repo, &task)?;
        removed += 1;
    }
    Ok(TaskWorktreeCleanup { removed })
}

fn clear_metadata_only_task_residue(repo: &Path) -> Result<TaskWorktreeCleanup> {
    let managed = managed_root(repo)?;
    if !managed.exists() {
        return Ok(TaskWorktreeCleanup { removed: 0 });
    }
    let managed = managed
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", managed.display()))?;
    let registered_worktrees = git_worktree_paths(repo)?
        .into_iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect::<BTreeSet<_>>();
    let mut removed = 0;
    for task in open_tasks(repo)? {
        if !task.task_worktree.exists() || !metadata_only_task_dir(&task.task_worktree)? {
            continue;
        }
        let worktree = task
            .task_worktree
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", task.task_worktree.display()))?;
        if !is_direct_child(&managed, &worktree) || registered_worktrees.contains(&worktree) {
            continue;
        }
        if !task_branch_is_missing(repo, &task.task_branch)? {
            continue;
        }
        fs::remove_dir_all(&task.task_worktree)
            .with_context(|| format!("failed to remove {}", task.task_worktree.display()))?;
        removed += 1;
    }
    Ok(TaskWorktreeCleanup { removed })
}

fn clear_finished_task_worktree(repo: &Path, task: &TaskEnv) -> Result<()> {
    if task.task_worktree.exists() {
        let _ = run_git(&task.task_worktree, ["checkout", "--detach"]);
        run_git(
            repo,
            [
                "worktree",
                "remove",
                "--force",
                path_arg(&task.task_worktree),
            ],
        )?;
        remove_metadata_only_task_dir(&task.task_worktree)?;
    }
    let _ = run_git(repo, ["branch", "-D", &task.task_branch]);
    Ok(())
}

fn clear_orphan_task_worktrees(repo: &Path) -> Result<TaskWorktreeCleanup> {
    let mut removed = 0;
    for orphan in orphan_task_worktrees(repo)? {
        run_git(repo, ["worktree", "remove", "--force", path_arg(&orphan)])?;
        removed += 1;
    }
    Ok(TaskWorktreeCleanup { removed })
}

fn orphan_task_worktrees(repo: &Path) -> Result<Vec<PathBuf>> {
    let managed = managed_root(repo)?;
    if !managed.exists() {
        return Ok(Vec::new());
    }
    let managed = managed
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", managed.display()))?;
    let worktrees = git_worktree_paths(repo)?;
    let mut orphans = Vec::new();
    for worktree in worktrees {
        if !worktree.exists() {
            continue;
        }
        let worktree = worktree
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", worktree.display()))?;
        if !is_direct_child(&managed, &worktree) {
            continue;
        }
        if worktree.join(".task/task.env").is_file() {
            continue;
        }
        orphans.push(worktree);
    }
    orphans.sort();
    Ok(orphans)
}

fn git_worktree_paths(repo: &Path) -> Result<Vec<PathBuf>> {
    Ok(git_output(repo, ["worktree", "list", "--porcelain"])?
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect())
}

fn metadata_only_task_dir(path: &Path) -> Result<bool> {
    if !path.is_dir() || !path.join(".task/task.env").is_file() {
        return Ok(false);
    }
    let mut found_task_dir = false;
    for entry in fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))? {
        let entry = entry?;
        if entry.file_name().to_string_lossy() != ".task" || !entry.file_type()?.is_dir() {
            return Ok(false);
        }
        found_task_dir = true;
    }
    Ok(found_task_dir)
}

fn remove_metadata_only_task_dir(path: &Path) -> Result<()> {
    if metadata_only_task_dir(path)? {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

fn task_branch_is_missing(repo: &Path, branch: &str) -> Result<bool> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Ok(false);
    }
    Ok(git_output(repo, ["branch", "--list", branch])?
        .trim()
        .is_empty())
}

fn is_direct_child(parent: &Path, child: &Path) -> bool {
    child
        .strip_prefix(parent)
        .ok()
        .is_some_and(|relative| relative.components().count() == 1)
}

fn prune_git_worktree_metadata(repo: &Path) -> Result<()> {
    run_git(repo, ["worktree", "prune"])
}

fn clear_stale_paused_tasks(repo: &Path, max_age_hours: u64) -> Result<StaleCleanup> {
    let now = unix_now();
    let max_age_seconds = max_age_hours.saturating_mul(60).saturating_mul(60);
    let mut removed = 0;
    for task in open_tasks(repo)? {
        if task.status != "paused" || !task_is_stale(&task, now, max_age_seconds) {
            continue;
        }
        run_git(
            repo,
            [
                "worktree",
                "remove",
                "--force",
                path_arg(&task.task_worktree),
            ],
        )?;
        let _ = run_git(repo, ["branch", "-D", &task.task_branch]);
        removed += 1;
    }
    Ok(StaleCleanup {
        max_age_hours,
        removed,
    })
}

fn task_is_stale(task: &TaskEnv, now: u64, max_age_seconds: u64) -> bool {
    let updated_at = task
        .updated_at
        .parse::<u64>()
        .ok()
        .or_else(|| task.started_at.parse::<u64>().ok())
        .unwrap_or(0);
    now.saturating_sub(updated_at) >= max_age_seconds
}

fn paused_task_ttl_hours() -> Result<u64> {
    match std::env::var("QCOLD_PAUSED_TASK_TTL_HOURS") {
        Ok(value) => value
            .parse::<u64>()
            .with_context(|| format!("invalid QCOLD_PAUSED_TASK_TTL_HOURS={value}")),
        Err(_) => Ok(DEFAULT_PAUSED_TASK_TTL_HOURS),
    }
}

fn clear_stale_bundles(repo: &Path, retention_hours: u64) -> Result<BundleCleanup> {
    let bundles = repo.join("bundles");
    if !bundles.is_dir() {
        return Ok(BundleCleanup {
            retention_hours,
            removed: 0,
        });
    }
    let now = SystemTime::now();
    let retention_seconds = retention_hours.saturating_mul(60).saturating_mul(60);
    let mut removed = 0;
    for entry in
        fs::read_dir(&bundles).with_context(|| format!("failed to read {}", bundles.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("zip") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);
        let age = now
            .duration_since(modified)
            .map_or(u64::MAX, |duration| duration.as_secs());
        if age < retention_seconds {
            continue;
        }
        fs::remove_file(&path).with_context(|| format!("failed to remove {}", path.display()))?;
        removed += 1;
    }
    Ok(BundleCleanup {
        retention_hours,
        removed,
    })
}

fn bundle_retention_hours() -> Result<u64> {
    match std::env::var("QCOLD_BUNDLE_RETENTION_HOURS") {
        Ok(value) => value
            .parse::<u64>()
            .with_context(|| format!("invalid QCOLD_BUNDLE_RETENTION_HOURS={value}")),
        Err(_) => Ok(DEFAULT_BUNDLE_RETENTION_HOURS),
    }
}
