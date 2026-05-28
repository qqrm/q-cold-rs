const CODEX_UPDATE_RESTART_RETRY: &str = "Codex updated and requested restart";

fn retry_after_queue_agent_launch_failure(agent_id: &str, message: &str) -> QueueItemOutcome {
    let cleanup = cleanup_queue_agent(agent_id);
    QueueItemOutcome::retryable_failure(format!("{message}; {cleanup}"))
}

fn retry_after_queue_agent_launch_update(agent_id: &str) -> QueueItemOutcome {
    let cleanup = cleanup_queue_agent(agent_id);
    QueueItemOutcome::retryable_failure(format!("{CODEX_UPDATE_RESTART_RETRY}; {cleanup}"))
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
