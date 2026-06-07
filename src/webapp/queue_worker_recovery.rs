const WEB_QUEUE_SEMANTIC_ITERATIONS_PER_ITEM: i64 = 3;
const WEB_QUEUE_AUTO_RECOVERY_ATTEMPTS: i64 = WEB_QUEUE_SEMANTIC_ITERATIONS_PER_ITEM - 1;

const QUEUE_AGENT_EXITED_BEFORE_CLOSEOUT: &str = "agent exited before task closeout";
const QUEUE_AGENT_EXITED_BEFORE_TASK_RECORD: &str = "agent exited before opening task record";
const QUEUE_AGENT_FAILED_QCOLD_CLOSEOUT: &str =
    "agent reached idle prompt after failed Q-COLD closeout";

fn queue_status_auto_recoverable(status: &str) -> bool {
    status == "closed:failed" || status == "failed-closeout"
}

fn queue_failure_message_auto_recoverable(message: &str) -> bool {
    matches!(
        message,
        QUEUE_AGENT_EXITED_BEFORE_CLOSEOUT | QUEUE_AGENT_FAILED_QCOLD_CLOSEOUT
    )
}

fn queue_failure_message_launch_recoverable(message: &str) -> bool {
    message == QUEUE_AGENT_EXITED_BEFORE_TASK_RECORD
        || message == QUEUE_LOCAL_LAUNCH_FAILED_BEFORE_TASK_RECORD_RELAUNCH_MESSAGE
        || message == REMOTE_NATIVE_MISSING_RECORD_RELAUNCH_MESSAGE
        || message.starts_with(
            "remote-native task record was not visible after remote-agent open",
        )
}

fn queue_task_status_terminal(status: &str) -> bool {
    status.starts_with("closed") || status == "failed-closeout"
}

fn queue_item_semantic_iteration(item: &state::QueueItemRow) -> i64 {
    item.recovery_attempts.max(0).saturating_add(1)
}

fn queue_item_recovery_active_or_pending(item: &state::QueueItemRow) -> bool {
    item.recovery_attempts > 0
        && (!queue_item_terminal(&item.status) || live_queue_item_recovery_agent_id(item).is_some())
}

fn queue_item_recovery_waiting_on_current_attempt(item: &state::QueueItemRow) -> bool {
    if item.recovery_attempts <= 0 {
        return false;
    }
    if live_queue_item_recovery_agent_id(item).is_some() {
        return true;
    }
    !queue_item_terminal(&item.status) && item.agent_id.is_none()
}

fn live_queue_item_recovery_agent_id(item: &state::QueueItemRow) -> Option<&str> {
    let agent_id = item.agent_id.as_deref()?;
    (item.recovery_attempts > 0
        && agent_running(agent_id)
        && !agent_terminal_closeout_failed(agent_id))
    .then_some(agent_id)
}

fn schedule_queue_item_auto_recovery(
    run_id: &str,
    item: &state::QueueItemRow,
    failure_message: &str,
) -> Result<bool> {
    let semantic_iterations = state::web_queue_item_semantic_iterations_started(item)?;
    if semantic_iterations >= WEB_QUEUE_SEMANTIC_ITERATIONS_PER_ITEM {
        return Ok(false);
    }
    let recovery_attempts = semantic_iterations;
    let message = format!("auto-recovery scheduled after failed task: {failure_message}");
    state::schedule_web_queue_item_recovery(
        run_id,
        &item.id,
        &message,
        failure_message,
        recovery_attempts,
    )?;
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
    let message = exhausted_queue_item_failure_message(item, message)?;
    state::update_web_queue_item(
        run_id,
        &item.id,
        "failed",
        &message,
        agent_id,
        attempts,
        None,
    )?;
    Ok(QueueItemOutcome::failed(message))
}

fn exhausted_queue_item_failure_message(
    item: &state::QueueItemRow,
    failure_message: &str,
) -> Result<String> {
    let semantic_iterations = state::web_queue_item_semantic_iterations_started(item)?;
    if semantic_iterations < WEB_QUEUE_SEMANTIC_ITERATIONS_PER_ITEM {
        return Ok(failure_message.to_string());
    }
    Ok(format!(
        "auto-recovery exhausted after {semantic_iterations} semantic iterations; \
         last failure: {failure_message}"
    ))
}
