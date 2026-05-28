const CODEX_UPDATE_RESTART_RETRY: &str = "Codex updated and requested restart";

fn retry_after_queue_agent_launch_failure(agent_id: &str, message: &str) -> QueueItemOutcome {
    let cleanup = cleanup_queue_agent(agent_id);
    QueueItemOutcome::retryable_failure(format!("{message}; {cleanup}"))
}

fn cleanup_stale_queue_agent_launch_artifacts(
    item: &state::QueueItemRow,
    launch_cwd: &Path,
) -> Result<()> {
    if queue_item_remote_native(item) {
        return Ok(());
    }
    let agent_id = queue_agent_id(item);
    if agent_running(&agent_id) {
        return Ok(());
    }
    if queue_agent_tmux_session_exists(&agent_id)? {
        bail!("queue agent terminal session qcold-{agent_id} still exists; refusing stale cleanup");
    }
    if !queue_launch_cwd_is_managed_task(launch_cwd)? {
        let worktree = agents::agent_worktree_path_for_launch_id(
            &agent_id,
            &queue_track(&item.run_id),
            0,
            launch_cwd,
        )
        .with_context(|| format!("failed to resolve queue agent worktree for {agent_id}"))?;
        if worktree.exists() {
            remove_clean_queue_agent_worktree(&worktree, launch_cwd)?;
        }
    }
    state::delete_agent_record(&agent_id)?;
    Ok(())
}

fn queue_launch_cwd_is_managed_task(launch_cwd: &Path) -> Result<bool> {
    let output = Command::new("git")
        .current_dir(launch_cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .with_context(|| format!("failed to inspect git root for {}", launch_cwd.display()))?;
    if !output.status.success() {
        return Ok(false);
    }
    let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    Ok(root.join(".task/task.env").is_file())
}

fn queue_agent_tmux_session_exists(agent_id: &str) -> Result<bool> {
    let session = format!("qcold-{agent_id}");
    match Command::new("tmux")
        .args(["has-session", "-t", &session])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) => Ok(status.success()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("failed to inspect tmux session {session}")),
    }
}

fn remove_clean_queue_agent_worktree(worktree: &Path, launch_cwd: &Path) -> Result<()> {
    let output = Command::new("git")
        .current_dir(worktree)
        .args([
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ])
        .output()
        .with_context(|| format!("failed to inspect queue agent worktree {}", worktree.display()))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "failed to inspect queue agent worktree {}: {}",
            worktree.display(),
            compact_process_output(&stdout, &stderr)
        );
    }
    if !output.stdout.is_empty() {
        bail!(
            "queue agent worktree {} has local changes; refusing stale cleanup",
            worktree.display()
        );
    }
    let worktree_arg = worktree.display().to_string();
    let output = Command::new("git")
        .current_dir(launch_cwd)
        .args(["worktree", "remove", "--force", &worktree_arg])
        .output()
        .with_context(|| format!("failed to remove queue agent worktree {}", worktree.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "failed to remove queue agent worktree {}: {}",
        worktree.display(),
        compact_process_output(&stdout, &stderr)
    );
}

fn queue_failure_retries_immediately(message: &str) -> bool {
    message.contains(CODEX_UPDATE_RESTART_RETRY)
}

fn handle_queue_launch_outcome(
    run_id: &str,
    item: &state::QueueItemRow,
    retries: &mut i64,
    outcome: QueueItemOutcome,
) -> Result<Option<QueueItemOutcome>> {
    match outcome {
        QueueItemOutcome::Failed {
            message,
            retryable: true,
        } if queue_failure_retries_immediately(&message)
            && retry_index(*retries) < WEB_QUEUE_RETRY_DELAYS.len() =>
        {
            *retries += 1;
            let retry_message = format!(
                "{message}; retry {}/{} now",
                *retries,
                WEB_QUEUE_RETRY_DELAYS.len()
            );
            state::update_web_queue_item(
                run_id,
                &item.id,
                "waiting",
                &retry_message,
                queue_launch_failure_agent_id(item).as_deref(),
                *retries,
                None,
            )?;
            Ok(None)
        }
        QueueItemOutcome::Failed {
            message,
            retryable: true,
        } if retry_index(*retries) < WEB_QUEUE_RETRY_DELAYS.len() => {
            let delay = WEB_QUEUE_RETRY_DELAYS[retry_index(*retries)];
            *retries += 1;
            let next_attempt_at = unix_now().saturating_add(delay);
            let retry_message = format!(
                "{message}; retry {}/{} in {}s",
                *retries,
                WEB_QUEUE_RETRY_DELAYS.len(),
                delay
            );
            state::update_web_queue_item(
                run_id,
                &item.id,
                "waiting",
                &retry_message,
                queue_launch_failure_agent_id(item).as_deref(),
                *retries,
                Some(next_attempt_at),
            )?;
            if sleep_queue_retry(run_id, delay)? {
                Ok(None)
            } else {
                Ok(Some(QueueItemOutcome::Stopped))
            }
        }
        QueueItemOutcome::Failed {
            message,
            retryable: true,
        } => {
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                &message,
                queue_launch_failure_agent_id(item).as_deref(),
                *retries,
                None,
            )?;
            Ok(Some(QueueItemOutcome::failed(message)))
        }
        outcome => Ok(Some(outcome)),
    }
}

fn queue_launch_failure_agent_id(item: &state::QueueItemRow) -> Option<String> {
    if queue_item_remote_native(item) {
        return Some(queue_agent_id(item));
    }
    item.agent_id.clone()
}

fn fail_remote_native_missing_task_record(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    let message = "remote-native task record was not visible after remote-agent open";
    state::update_web_queue_item(
        run_id,
        &item.id,
        "failed",
        message,
        Some(agent_id),
        attempts,
        None,
    )?;
    Ok(QueueItemOutcome::failed(message))
}

fn cleanup_queue_agent(agent_id: &str) -> String {
    let was_running = agent_running(agent_id);
    let terminate = if was_running {
        agents::terminate_agent(agent_id)
    } else {
        Ok(false)
    };
    let can_delete_record = terminate.is_ok() || !agent_running(agent_id);
    let deleted = if can_delete_record {
        state::delete_agent_record(agent_id)
    } else {
        Ok(false)
    };
    match (terminate, deleted) {
        (Ok(true), Ok(true)) => "agent terminal closed; agent record deleted".to_string(),
        (Ok(true), Ok(false)) => "agent terminal closed".to_string(),
        (Ok(false), Ok(true)) => "agent already stopped; agent record deleted".to_string(),
        (Ok(false), Ok(false)) => "agent already stopped".to_string(),
        (Ok(_), Err(err)) => format!("agent terminal closed; agent record cleanup failed: {err:#}"),
        (Err(err), Ok(true)) => {
            format!("agent cleanup failed: {err:#}; stale agent record deleted")
        }
        (Err(err), Ok(false)) => {
            format!("agent cleanup failed: {err:#}; stale agent record delete skipped")
        }
        (Err(err), Err(delete_err)) => {
            format!("agent cleanup failed: {err:#}; agent record cleanup failed: {delete_err:#}")
        }
    }
}
