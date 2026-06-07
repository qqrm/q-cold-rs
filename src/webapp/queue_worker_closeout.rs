fn wait_for_queue_item_closeout(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
    lease: Option<&state::QueueWorkerLease>,
) -> Result<QueueItemOutcome> {
    loop {
        if state::web_queue_stop_requested(run_id)? {
            pause_web_queue_item(run_id, item, Some(agent_id), attempts)?;
            return Ok(QueueItemOutcome::Stopped);
        }
        heartbeat_queue_item_worker_lease(lease)?;
        thread::sleep(Duration::from_secs(5));
        crate::sync_codex_task_records().ok();
        let run = status_reducer_run_view(run_id, item);
        let evidence = collect_queue_status_evidence_for_item(&run, item)?;
        let reduction = reduce_queue_status(&evidence);
        if let Some(status) = evidence.task_status.as_deref() {
            if let Some(outcome) = queue_item_closeout_outcome_from_reduction(
                run_id,
                item,
                agent_id,
                attempts,
                status,
                &evidence,
                &reduction,
            )? {
                return Ok(outcome);
            }
        } else if reduction.allowed_action == QueueAllowedAction::BoundedRelaunch {
            if queue_item_remote_native(item) {
                if let Some(outcome) =
                    missing_queue_task_record_outcome(run_id, item, agent_id, attempts)?
                {
                    return Ok(outcome);
                }
                continue;
            }
            return bounded_queue_item_relaunch(
                run_id,
                item,
                queue_launch_failed_before_record_message(item),
                attempts,
            );
        } else if reduction.allowed_action == QueueAllowedAction::RefreshEvidence {
            update_queue_item_status_sync_unavailable_wait(
                run_id,
                item,
                Some(agent_id),
                attempts,
                reduction.reason,
            )?;
        } else if let Some(outcome) =
            missing_queue_task_record_outcome(run_id, item, agent_id, attempts)?
        {
            return Ok(outcome);
        }
    }
}

fn update_queue_item_status_sync_unavailable_wait(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: Option<&str>,
    attempts: i64,
    reason: &str,
) -> Result<Option<QueueItemOutcome>> {
    let message = format!("{reason}; checked_at={}", unix_now());
    update_queue_item_unless_terminal(
        run_id,
        &item.id,
        "running",
        &message,
        agent_id,
        attempts,
        None,
    )
}

fn heartbeat_queue_item_worker_lease(lease: Option<&state::QueueWorkerLease>) -> Result<()> {
    let Some(lease) = lease else {
        return Ok(());
    };
    if state::heartbeat_web_queue_item_worker_lease(lease, WEB_QUEUE_WORKER_LEASE_TTL_SECS)? {
        return Ok(());
    }
    match state::inspect_web_queue_item_worker_lease(&lease.run_id, &lease.item_id)? {
        state::QueueWorkerLeaseState::Terminal { .. }
        | state::QueueWorkerLeaseState::Retryable { .. } => Ok(()),
        state => bail!(
            "queue worker lease lost for {}:{} to {:?}",
            lease.run_id,
            lease.item_id,
            state
        ),
    }
}

fn missing_queue_task_record_outcome(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
) -> Result<Option<QueueItemOutcome>> {
    if queue_item_remote_native(item) {
        if let Some(outcome) = latest_queue_item_terminal_outcome(run_id, &item.id)? {
            return Ok(Some(outcome));
        }
        if remote_native_session_running(item, agent_id) {
            return update_remote_native_missing_record_wait(run_id, item, agent_id, attempts);
        }
        return fail_remote_native_missing_task_record(run_id, item, agent_id, attempts).map(Some);
    }
    if agent_running(agent_id) {
        let _ = submit_agent_terminal_pending_paste(agent_id);
        return Ok(None);
    }
    bounded_queue_item_relaunch(
        run_id,
        item,
        QUEUE_LOCAL_LAUNCH_FAILED_BEFORE_TASK_RECORD_RELAUNCH_MESSAGE,
        attempts,
    )
    .map(Some)
}

fn queue_launch_failed_before_record_message(item: &state::QueueItemRow) -> &'static str {
    if queue_item_remote_native(item) {
        REMOTE_NATIVE_MISSING_RECORD_RELAUNCH_MESSAGE
    } else {
        QUEUE_LOCAL_LAUNCH_FAILED_BEFORE_TASK_RECORD_RELAUNCH_MESSAGE
    }
}

fn bounded_queue_item_relaunch(
    run_id: &str,
    item: &state::QueueItemRow,
    message: &str,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    if queue_item_remote_native(item)
        && item
            .remote_launcher
            .as_deref()
            .is_none_or(|launcher| launcher.trim().is_empty())
    {
        let message = format!("{message}; remote launcher unavailable");
        state::update_web_queue_item(
            run_id,
            &item.id,
            "failed",
            &message,
            item.agent_id.as_deref(),
            attempts,
            None,
        )?;
        state::update_web_queue_run(run_id, "failed", item.position, &message)?;
        return Ok(QueueItemOutcome::failed(message));
    }
    if retry_index(attempts) < WEB_QUEUE_RETRY_DELAYS.len() {
        let delay = WEB_QUEUE_RETRY_DELAYS[retry_index(attempts)];
        let attempts = attempts.saturating_add(1);
        let next_attempt_at = unix_now().saturating_add(delay);
        state::schedule_web_queue_item_relaunch(
            run_id,
            &item.id,
            message,
            attempts,
            next_attempt_at,
        )?;
        state::update_web_queue_run(run_id, "running", item.position, message)?;
        return Ok(QueueItemOutcome::RecoveryScheduled);
    }
    let message = format!(
        "launch retries exhausted after {} attempts; {message}",
        WEB_QUEUE_RETRY_DELAYS.len()
    );
    state::update_web_queue_item(
        run_id,
        &item.id,
        "failed",
        &message,
        item.agent_id.as_deref(),
        attempts,
        None,
    )?;
    state::update_web_queue_run(run_id, "failed", item.position, &message)?;
    Ok(QueueItemOutcome::failed(message))
}

fn update_remote_native_missing_record_wait(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
) -> Result<Option<QueueItemOutcome>> {
    update_queue_item_unless_terminal(
        run_id,
        &item.id,
        "running",
        "waiting for remote-native task record visibility after remote-agent open",
        Some(agent_id),
        attempts,
        None,
    )
}

fn pause_web_queue_item(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: Option<&str>,
    attempts: i64,
) -> Result<()> {
    state::update_web_queue_item(
        run_id,
        &item.id,
        "stopped",
        "stopped by operator; press Continue to resume",
        agent_id,
        attempts,
        None,
    )
}

fn update_successful_queue_item(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: Option<&str>,
    attempts: i64,
) -> Result<()> {
    let message = agent_id.map_or_else(
        || "closed successfully".to_string(),
        |agent_id| format!("closed successfully; {}", cleanup_queue_executor(item, agent_id)),
    );
    state::update_web_queue_item(
        run_id,
        &item.id,
        "success",
        &message,
        agent_id,
        attempts,
        None,
    )
}

fn cleanup_queue_executor(item: &state::QueueItemRow, agent_id: &str) -> String {
    if !queue_item_remote_native(item) {
        return cleanup_queue_agent(agent_id);
    }
    let repo_root = match queue_item_repo_root(item) {
        Ok(repo_root) => repo_root,
        Err(err) => return format!("remote-agent cleanup skipped: {err:#}"),
    };
    let session = remote_native_queue_session(agent_id);
    match run_remote_agent_contract(item, &repo_root, "down", &session, None, None) {
        Ok(()) => "remote-agent session stopped".to_string(),
        Err(err) => format!("remote-agent cleanup failed: {err:#}"),
    }
}
