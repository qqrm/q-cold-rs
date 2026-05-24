const TASK_BUNDLE_SUMMARY_PATH: &str = "summary.md";
const TASK_BUNDLE_METADATA_DIR: &str = "metadata";
const TASK_BUNDLE_LOGS_DIR: &str = "logs";
const TASK_BUNDLE_EVIDENCE_DIR: &str = "evidence";
const TASK_BUNDLE_REPO_DIR: &str = "repo";
const TERMINAL_RECEIPT_PATH: &str = "metadata/terminal-receipt.env";

fn create_task_archive_bundle(task: &TaskEnv) -> Result<PathBuf> {
    let bundles = task.primary_repo_path.join("bundles");
    fs::create_dir_all(&bundles)?;
    let bundle = bundles.join(format!("{}-{}.zip", task.task_name, unix_now()));
    let staging = unique_task_bundle_staging("qcold-task-bundle");
    fs::create_dir_all(&staging)?;

    let result = (|| {
        let metadata = staging.join(TASK_BUNDLE_METADATA_DIR);
        let logs = staging.join(TASK_BUNDLE_LOGS_DIR);
        let evidence = staging.join(TASK_BUNDLE_EVIDENCE_DIR);
        let repo = staging.join(TASK_BUNDLE_REPO_DIR);
        fs::create_dir_all(&metadata)?;
        fs::create_dir_all(&logs)?;
        fs::create_dir_all(&evidence)?;
        fs::create_dir_all(&repo)?;

        let task_head = git_output(&task.task_worktree, ["rev-parse", "HEAD"])
            .unwrap_or_else(|_| task.task_head.clone());
        let dirty = if task_status_short(&task.task_worktree).is_empty() {
            "clean"
        } else {
            "dirty"
        };
        write_task_bundle_env(&metadata, task, &task_head, dirty)?;
        write_task_bundle_manifest(&metadata)?;
        write_preliminary_task_summary(&staging, task, &bundle, &task_head, dirty)?;
        write_task_bundle_evidence(&evidence, task)?;
        copy_task_metadata(task, &metadata)?;
        copy_task_logs(task, &logs)?;
        copy_repo_snapshot(&task.task_worktree, &repo)?;

        create_zip_from_stage(&staging, &bundle).context("failed to create terminal evidence bundle")
    })();

    fs::remove_dir_all(&staging).ok();
    result?;
    Ok(bundle)
}

fn add_terminal_receipt_to_bundle(bundle: &Path, receipt: &TerminalReceipt<'_>) -> Result<()> {
    let staging = unique_task_bundle_staging("qcold-terminal-receipt");
    let metadata = staging.join(TASK_BUNDLE_METADATA_DIR);
    fs::create_dir_all(&metadata)?;
    fs::write(
        metadata.join("terminal-receipt.env"),
        render_terminal_receipt(receipt),
    )?;
    fs::write(
        staging.join(TASK_BUNDLE_SUMMARY_PATH),
        render_terminal_summary(receipt, bundle),
    )?;
    let status = Command::new("7z")
        .current_dir(&staging)
        .args([
            "a",
            "-tzip",
            path_arg(bundle),
            TASK_BUNDLE_SUMMARY_PATH,
            TERMINAL_RECEIPT_PATH,
        ])
        .status()
        .context("failed to append terminal receipt to bundle")?;
    fs::remove_dir_all(&staging).ok();
    if !status.success() {
        bail!("7z failed to append terminal receipt with status {status}");
    }
    Ok(())
}

fn create_zip_from_stage(stage: &Path, bundle: &Path) -> Result<()> {
    let status = Command::new("7z")
        .current_dir(stage)
        .args([
            "a",
            "-tzip",
            path_arg(bundle),
            TASK_BUNDLE_SUMMARY_PATH,
            TASK_BUNDLE_METADATA_DIR,
            TASK_BUNDLE_LOGS_DIR,
            TASK_BUNDLE_EVIDENCE_DIR,
            TASK_BUNDLE_REPO_DIR,
        ])
        .status()
        .context("failed to create task bundle ZIP")?;
    if !status.success() {
        bail!("7z failed to create terminal evidence bundle with status {status}");
    }
    Ok(())
}

fn unique_task_bundle_staging(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

fn task_status_short(repo: &Path) -> String {
    git_output(repo, ["status", "--porcelain", "--untracked-files=all"]).unwrap_or_default()
}

fn write_task_bundle_env(metadata: &Path, task: &TaskEnv, task_head: &str, dirty: &str) -> Result<()> {
    let mut output = String::new();
    for (key, value) in [
        ("TASK_ID", task.task_id.as_str()),
        ("TASK_NAME", task.task_name.as_str()),
        ("TASK_SEQUENCE", task.task_sequence.as_str()),
        ("TASK_BRANCH", task.task_branch.as_str()),
        ("TASK_EXECUTION_ANCHOR", task.task_execution_anchor.as_str()),
        ("TASK_DESCRIPTION", task.task_description.as_str()),
        ("TASK_WORKTREE", &task.task_worktree.display().to_string()),
        ("PRIMARY_REPO_PATH", &task.primary_repo_path.display().to_string()),
        ("BASE_BRANCH", task.base_branch.as_str()),
        ("BASE_HEAD", task.base_head.as_str()),
        ("TASK_HEAD", task_head),
        ("STARTED_AT", task.started_at.as_str()),
        ("UPDATED_AT", task.updated_at.as_str()),
        ("STATUS", task.status.as_str()),
        ("DIRTY_STATE", dirty),
        ("DEVCONTAINER_NAME", task.devcontainer_name.as_str()),
        ("DELIVERY_MODE", task.delivery_mode.as_str()),
        ("CODEX_THREAD_ID", task.codex_thread_id.as_str()),
        (
            "QCOLD_OUTPUT_GUARD_ENABLED",
            task.output_guard_enabled.as_str(),
        ),
        (
            "QCOLD_OUTPUT_GUARD_COMMANDS",
            task.output_guard_commands.as_str(),
        ),
    ] {
        output.push_str(key);
        output.push('=');
        output.push_str(&shell_quote(value));
        output.push('\n');
    }
    output.push_str("BUNDLE_CREATED_UNIX=");
    output.push_str(&unix_now().to_string());
    output.push('\n');
    fs::write(metadata.join("bundle.env"), output)?;
    Ok(())
}

fn write_task_bundle_manifest(metadata: &Path) -> Result<()> {
    fs::write(
        metadata.join("bundle-manifest.txt"),
        "BUNDLE_KIND=task-evidence\n\
         ARCHIVE_FORMAT=zip\n\
         ARCHIVE_TOOL=7z\n\
         SUMMARY_PATH=summary.md\n\
         TASK_METADATA_ROOT=metadata/\n\
         TASK_LOG_ROOT=logs/\n\
         TASK_EVIDENCE_ROOT=evidence/\n\
         REPO_SNAPSHOT_ROOT=repo/\n\
         REPO_SNAPSHOT_SELECTION=tracked plus untracked non-ignored git-visible paths\n\
         SKIPPED_PATH=.git\n\
         SKIPPED_PATH=.task\n\
         SKIPPED_PATH=bundles\n\
         SKIPPED_PATH=target\n\
         SKIPPED_PATH=build\n\
         SKIPPED_PATH=dist\n\
         SKIPPED_PATH=node_modules\n",
    )?;
    Ok(())
}

fn write_preliminary_task_summary(
    stage: &Path,
    task: &TaskEnv,
    bundle: &Path,
    task_head: &str,
    dirty: &str,
) -> Result<()> {
    let mut summary = String::new();
    summary.push_str("# Task Summary\n\n");
    push_summary_line(&mut summary, "Task", &format!("`{}`", task.task_id));
    push_summary_line(&mut summary, "Name", &format!("`{}`", task.task_name));
    push_summary_line(&mut summary, "Branch", &format!("`{}`", task.task_branch));
    push_summary_line(&mut summary, "Status", &format!("`{}`", task.status));
    push_summary_line(&mut summary, "Task head", &format!("`{task_head}`"));
    push_summary_line(&mut summary, "Dirty state", &format!("`{dirty}`"));
    push_summary_line(&mut summary, "Bundle", &format!("`{}`", bundle.display()));
    summary.push_str(
        "\nThis summary was generated before terminal receipt metadata was finalized. \
         Prefer `metadata/terminal-receipt.env` when it is present.\n",
    );
    fs::write(stage.join(TASK_BUNDLE_SUMMARY_PATH), summary)?;
    Ok(())
}

fn write_task_bundle_evidence(evidence: &Path, task: &TaskEnv) -> Result<()> {
    fs::write(
        evidence.join("git-status.txt"),
        git_output(
            &task.task_worktree,
            ["status", "--short", "--branch", "--untracked-files=all"],
        )
        .unwrap_or_default(),
    )?;
    fs::write(
        evidence.join("working-tree.patch"),
        git_output(&task.task_worktree, ["diff", "--binary"]).unwrap_or_default(),
    )?;
    fs::write(
        evidence.join("index.patch"),
        git_output(&task.task_worktree, ["diff", "--binary", "--cached"]).unwrap_or_default(),
    )?;
    Ok(())
}

fn copy_task_metadata(task: &TaskEnv, metadata: &Path) -> Result<()> {
    let task_env = task.task_worktree.join(".task/task.env");
    if task_env.is_file() {
        fs::copy(task_env, metadata.join("task.env"))?;
    }
    Ok(())
}

fn copy_task_logs(task: &TaskEnv, logs: &Path) -> Result<()> {
    let source = task.task_worktree.join(".task/logs");
    if source.is_dir() {
        copy_dir_all(&source, logs)?;
    }
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let target = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_all(&source, &target)?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source, target)?;
        }
    }
    Ok(())
}

fn copy_repo_snapshot(repo: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for rel in git_visible_snapshot_paths(repo)? {
        if should_skip_snapshot_path(&rel) {
            continue;
        }
        let source = repo.join(&rel);
        let Ok(metadata) = fs::symlink_metadata(&source) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let target = dst.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, target)?;
    }
    Ok(())
}

fn git_visible_snapshot_paths(repo: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut selected = BTreeSet::new();
    for path in git_output(repo, ["ls-files", "--cached", "-z"])?
        .split('\0')
        .filter(|path| !path.is_empty())
    {
        selected.insert(PathBuf::from(path));
    }
    for path in git_output(repo, ["ls-files", "--others", "--exclude-standard", "-z"])?
        .split('\0')
        .filter(|path| !path.is_empty())
    {
        selected.insert(PathBuf::from(path));
    }
    Ok(selected)
}

fn should_skip_snapshot_path(path: &Path) -> bool {
    path.components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
        .is_some_and(|component| {
            matches!(
                component,
                ".git" | ".task" | "bundles" | "target" | "build" | "dist" | "node_modules"
            ) || component.starts_with("build-")
        })
}

fn render_terminal_summary(receipt: &TerminalReceipt<'_>, bundle: &Path) -> String {
    let mut summary = String::new();
    summary.push_str("# Task Summary\n\n");
    push_summary_line(&mut summary, "Outcome", &format!("`{}`", receipt.outcome));
    push_summary_line(
        &mut summary,
        "Closeout category",
        &format!("`{}`", receipt.closeout_category),
    );
    push_summary_line(
        &mut summary,
        "Current flow problem",
        &format!("`{}`", receipt.current_flow_problem),
    );
    push_summary_line(
        &mut summary,
        "Historical flow problem",
        &format!("`{}`", receipt.historical_flow_problem),
    );
    if let Some(reason) = receipt.reason.filter(|reason| !reason.trim().is_empty()) {
        push_summary_line(&mut summary, "Reason", reason.trim());
    }
    if let Some(phase) = receipt
        .closeout_failure_phase
        .filter(|phase| !phase.trim().is_empty())
    {
        push_summary_line(&mut summary, "Failure phase", &format!("`{}`", phase.trim()));
    }
    push_summary_line(
        &mut summary,
        "Primary checkout clean",
        if receipt.primary_clean { "`yes`" } else { "`no`" },
    );
    push_summary_line(
        &mut summary,
        "Task worktree removed",
        if receipt.worktree_removed {
            "`yes`"
        } else {
            "`no`"
        },
    );
    push_summary_line(
        &mut summary,
        "Local task branch removed",
        if receipt.branch_removed { "`yes`" } else { "`no`" },
    );
    push_summary_line(
        &mut summary,
        "Task dirty files",
        &format!("`{}`", receipt.task_status.dirty_file_count),
    );
    push_summary_line(
        &mut summary,
        "Task conflicts",
        &format!("`{}`", receipt.task_status.conflict_file_count),
    );
    push_summary_line(&mut summary, "Bundle", &format!("`{}`", bundle.display()));
    summary.push_str(
        "\nDetailed machine-readable metadata lives in `metadata/bundle.env` and \
         `metadata/terminal-receipt.env`.\n",
    );
    summary
}

fn push_summary_line(summary: &mut String, key: &str, value: &str) {
    summary.push_str("- ");
    summary.push_str(key);
    summary.push_str(": ");
    summary.push_str(value);
    summary.push('\n');
}

fn render_terminal_receipt(receipt: &TerminalReceipt<'_>) -> String {
    let mut output = String::new();
    let primary_clean = yes_no(receipt.primary_clean);
    let worktree_removed = yes_no(receipt.worktree_removed);
    let branch_removed = yes_no(receipt.branch_removed);
    let primary_dirty_file_count = receipt.primary_status.dirty_file_count.to_string();
    let primary_conflict_file_count = receipt.primary_status.conflict_file_count.to_string();
    let primary_conflict_paths = receipt.primary_status.conflict_paths.join("\n");
    let dirty_file_count = receipt.task_status.dirty_file_count.to_string();
    let conflict_file_count = receipt.task_status.conflict_file_count.to_string();
    let conflict_paths = receipt.task_status.conflict_paths.join("\n");
    for (key, value) in [
        ("OUTCOME", receipt.outcome.to_string()),
        ("REASON", receipt.reason.unwrap_or("").to_string()),
        ("CLOSEOUT_CATEGORY", receipt.closeout_category.to_string()),
        (
            "CURRENT_FLOW_PROBLEM",
            receipt.current_flow_problem.to_string(),
        ),
        (
            "HISTORICAL_FLOW_PROBLEM",
            receipt.historical_flow_problem.to_string(),
        ),
        (
            "CLOSEOUT_FAILURE_PHASE",
            receipt.closeout_failure_phase.unwrap_or("").to_string(),
        ),
        (
            "CLOSEOUT_FAILURE_ERROR",
            receipt.closeout_failure_error.unwrap_or("").to_string(),
        ),
        ("PRIMARY_CHECKOUT_CLEAN", primary_clean),
        (
            "PRIMARY_CHECKOUT_STATUS_SHORT",
            receipt.primary_status.status_short.clone(),
        ),
        (
            "PRIMARY_CHECKOUT_DIRTY_FILE_COUNT",
            primary_dirty_file_count,
        ),
        (
            "PRIMARY_CHECKOUT_CONFLICT_FILE_COUNT",
            primary_conflict_file_count,
        ),
        ("PRIMARY_CHECKOUT_CONFLICTS", primary_conflict_paths),
        ("TASK_WORKTREE_REMOVED", worktree_removed),
        ("LOCAL_TASK_BRANCH_REMOVED", branch_removed),
        (
            "TASK_WORKTREE_STATUS_SHORT",
            receipt.task_status.status_short.clone(),
        ),
        ("TASK_WORKTREE_DIRTY_FILE_COUNT", dirty_file_count),
        ("TASK_WORKTREE_CONFLICT_FILE_COUNT", conflict_file_count),
        ("TASK_WORKTREE_CONFLICTS", conflict_paths),
        ("CANONICAL_VALIDATION", "not-applicable".to_string()),
    ] {
        output.push_str(key);
        output.push('=');
        output.push_str(&shell_quote(&value));
        output.push('\n');
    }
    output
}

fn yes_no(value: bool) -> String {
    String::from(if value { "yes" } else { "no" })
}
