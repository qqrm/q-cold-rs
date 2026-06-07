fn reconcile_queue_item_without_task_record(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    reduction: &QueueStatusReduction,
    terminal_run: &mut Option<(String, i64, String)>,
) -> Result<Option<bool>> {
    match reduction.allowed_action {
        QueueAllowedAction::BoundedRelaunch => {
            let outcome = bounded_queue_item_relaunch(
                &run.id,
                item,
                queue_launch_failed_before_record_message(item),
                item.attempts,
            )?;
            if let QueueItemOutcome::Failed { message, .. } = outcome {
                terminal_run.get_or_insert(("failed".into(), item.position, message));
            }
            Ok(Some(true))
        }
        QueueAllowedAction::RefreshEvidence => {
            if item.status.is_starting_or_running() {
                update_queue_item_status_sync_unavailable_wait(
                    &run.id,
                    item,
                    item.agent_id.as_deref(),
                    item.attempts,
                    reduction.reason,
                )?;
            }
            Ok(Some(false))
        }
        _ => Ok(None),
    }
}

fn reconcile_queue_item_fallback_status(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    terminal_run: &mut Option<(String, i64, String)>,
) -> Result<bool> {
    if item.status.is_failed_or_blocked()
        && queue_failure_message_launch_recoverable(&item.message)
    {
        let outcome = bounded_queue_item_relaunch(
            &run.id,
            item,
            queue_launch_failed_before_record_message(item),
            item.attempts,
        )?;
        if let QueueItemOutcome::Failed { message, .. } = outcome {
            terminal_run.get_or_insert(("failed".into(), item.position, message));
        }
        return Ok(true);
    }
    if item.status.is_failed_or_blocked()
        && queue_failure_message_auto_recoverable(&item.message)
        && schedule_queue_item_auto_recovery(&run.id, item, &item.message)?
    {
        return Ok(true);
    }
    if let Some(agent_id) = item.agent_id.as_deref() {
        if let Some(message) = queue_agent_failure_message(item, agent_id) {
            if schedule_queue_item_auto_recovery(&run.id, item, message)? {
                return Ok(true);
            }
            state::update_web_queue_item(
                &run.id,
                &item.id,
                "failed",
                message,
                Some(agent_id),
                item.attempts,
                None,
            )?;
            terminal_run.get_or_insert(("failed".into(), item.position, message.to_string()));
            return Ok(true);
        }
    }
    if item.status.is_success() && item.agent_id.as_deref().is_some_and(agent_running) {
        update_successful_queue_item(&run.id, item, item.agent_id.as_deref(), item.attempts)?;
        return Ok(true);
    }
    Ok(false)
}
