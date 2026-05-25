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
