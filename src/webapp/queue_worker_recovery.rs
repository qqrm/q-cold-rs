const WEB_QUEUE_AUTO_RECOVERY_ATTEMPTS: i64 = 1;

fn queue_status_auto_recoverable(status: &str) -> bool {
    status == "closed:failed"
}

fn queue_task_status_terminal(status: &str) -> bool {
    status.starts_with("closed") || status == "failed-closeout"
}

fn queue_item_recovery_active_or_pending(item: &state::QueueItemRow) -> bool {
    item.recovery_attempts > 0 && !queue_item_terminal(&item.status)
}

fn queue_item_recovery_waiting_on_current_attempt(item: &state::QueueItemRow) -> bool {
    queue_item_recovery_active_or_pending(item)
        && item
            .agent_id
            .as_deref()
            .is_none_or(|agent_id| agent_running(agent_id) && !agent_terminal_closeout_failed(agent_id))
}

fn schedule_queue_item_auto_recovery(
    run_id: &str,
    item: &state::QueueItemRow,
    failure_message: &str,
) -> Result<bool> {
    if item.recovery_attempts >= WEB_QUEUE_AUTO_RECOVERY_ATTEMPTS {
        return Ok(false);
    }
    let recovery_attempts = item.recovery_attempts.saturating_add(1);
    let message = format!("auto-recovery scheduled after failed task: {failure_message}");
    state::schedule_web_queue_item_recovery(run_id, &item.id, &message, recovery_attempts)?;
    state::update_web_queue_run(run_id, "running", item.position, &message)?;
    Ok(true)
}

fn fail_or_schedule_queue_item_recovery(
    run_id: &str,
    item: &state::QueueItemRow,
    message: &str,
    agent_id: Option<&str>,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    if schedule_queue_item_auto_recovery(run_id, item, message)? {
        return Ok(QueueItemOutcome::RecoveryScheduled);
    }
    state::update_web_queue_item(
        run_id,
        &item.id,
        "failed",
        message,
        agent_id,
        attempts,
        None,
    )?;
    Ok(QueueItemOutcome::failed(message))
}
