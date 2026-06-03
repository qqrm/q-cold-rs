const REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE: &str =
    "remote-native task is open, but remote-agent session is not running on the remote host";

fn reconcile_queue_task_record_status(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    status: String,
    changed: &mut bool,
    terminal_run: &mut Option<(String, i64, String)>,
) -> Result<bool> {
    if status == "closed:success" {
        if item.status != "success" || item.agent_id.as_deref().is_some_and(agent_running) {
            update_successful_queue_item(&run.id, item, item.agent_id.as_deref(), item.attempts)?;
            *changed = true;
        }
        return Ok(true);
    }
    if remote_native_stopped_open_record_with_live_session(item, &status) {
        let agent_id = item.agent_id.as_deref();
        let message = agent_id.map_or_else(
            || "remote-native open task resumed".to_string(),
            |agent_id| format!("resumed remote-native agent {agent_id}"),
        );
        state::update_web_queue_item(
            &run.id,
            &item.id,
            "running",
            &message,
            agent_id,
            item.attempts,
            None,
        )?;
        state::update_web_queue_run(&run.id, "running", item.position, "running")?;
        *changed = true;
        return Ok(true);
    }
    if remote_native_open_record_without_live_session(item, &status) {
        state::update_web_queue_item(
            &run.id,
            &item.id,
            "stopped",
            REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE,
            item.agent_id.as_deref(),
            item.attempts,
            None,
        )?;
        *changed = true;
        terminal_run.get_or_insert((
            "stopped".to_string(),
            item.position,
            REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE.to_string(),
        ));
        return Ok(true);
    }
    if status == "paused" && item.status != "paused" {
        state::update_web_queue_item(
            &run.id,
            &item.id,
            "paused",
            &status,
            item.agent_id.as_deref(),
            item.attempts,
            None,
        )?;
        *changed = true;
        terminal_run.get_or_insert(("stopped".to_string(), item.position, status));
        return Ok(true);
    }
    if queue_status_auto_recoverable(&status)
        && queue_item_recovery_waiting_on_current_attempt(item)
    {
        return Ok(true);
    }
    if queue_status_auto_recoverable(&status)
        && schedule_queue_item_auto_recovery(&run.id, item, &status)?
    {
        *changed = true;
        return Ok(true);
    }
    if queue_task_status_terminal(&status) && item.status != "success" {
        state::update_web_queue_item(
            &run.id,
            &item.id,
            "failed",
            &status,
            item.agent_id.as_deref(),
            item.attempts,
            None,
        )?;
        *changed = true;
        terminal_run.get_or_insert(("failed".to_string(), item.position, status));
        return Ok(true);
    }
    Ok(false)
}

fn remote_native_open_record_without_live_session(
    item: &state::QueueItemRow,
    status: &str,
) -> bool {
    status == "open"
        && queue_item_remote_native(item)
        && matches!(item.status.as_str(), "starting" | "running")
        && item
            .agent_id
            .as_deref()
            .is_some_and(|agent_id| !remote_native_session_running(item, agent_id))
}

fn remote_native_stopped_open_record_with_live_session(
    item: &state::QueueItemRow,
    status: &str,
) -> bool {
    status == "open"
        && queue_item_remote_native(item)
        && matches!(item.status.as_str(), "stopped" | "paused")
        && item
            .agent_id
            .as_deref()
            .is_some_and(|agent_id| remote_native_session_running(item, agent_id))
}

fn queue_item_status_closeout_outcome(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
    status: String,
) -> Result<Option<QueueItemOutcome>> {
    if status == "closed:success" {
        update_successful_queue_item(run_id, item, Some(agent_id), attempts)?;
        return Ok(Some(QueueItemOutcome::Success));
    }
    if status == "paused" {
        return mark_queue_item_paused(run_id, item, agent_id, attempts, &status).map(Some);
    }
    if queue_status_auto_recoverable(&status)
        && queue_item_recovery_active_or_pending(item)
        && agent_running(agent_id)
        && !agent_terminal_closeout_failed(agent_id)
    {
        return Ok(None);
    }
    if queue_status_auto_recoverable(&status) {
        return fail_or_schedule_queue_item_recovery(
            run_id,
            item,
            &status,
            Some(agent_id),
            attempts,
        )
        .map(Some);
    }
    if queue_task_status_terminal(&status) {
        return fail_queue_item_from_task_status(run_id, item, agent_id, attempts, status).map(Some);
    }
    if remote_native_stopped_open_record_with_live_session(item, &status) {
        state::update_web_queue_item(
            run_id,
            &item.id,
            "running",
            &format!("resumed remote-native agent {agent_id}"),
            Some(agent_id),
            attempts,
            None,
        )?;
        state::update_web_queue_run(run_id, "running", item.position, "running")?;
        return Ok(None);
    }
    if remote_native_open_record_without_live_session(item, &status) {
        return stop_remote_native_disconnected_item(run_id, item, agent_id, attempts).map(Some);
    }
    if status == "open" && !queue_item_remote_native(item) && !agent_running(agent_id) {
        return fail_or_schedule_queue_item_recovery(
            run_id,
            item,
            "agent exited before task closeout",
            Some(agent_id),
            attempts,
        )
        .map(Some);
    }
    if status == "open"
        && !queue_item_remote_native(item)
        && submit_agent_terminal_pending_paste(agent_id).unwrap_or(false)
    {
        return Ok(None);
    }
    if status == "open"
        && !queue_item_remote_native(item)
        && agent_terminal_closeout_failed(agent_id)
    {
        return fail_or_schedule_queue_item_recovery(
            run_id,
            item,
            "agent reached idle prompt after failed Q-COLD closeout",
            Some(agent_id),
            attempts,
        )
        .map(Some);
    }
    Ok(None)
}

fn mark_queue_item_paused(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
    status: &str,
) -> Result<QueueItemOutcome> {
    state::update_web_queue_item(
        run_id,
        &item.id,
        "paused",
        status,
        Some(agent_id),
        attempts,
        None,
    )?;
    state::update_web_queue_run(run_id, "stopped", item.position, status)?;
    Ok(QueueItemOutcome::Stopped)
}

fn fail_queue_item_from_task_status(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
    status: String,
) -> Result<QueueItemOutcome> {
    state::update_web_queue_item(
        run_id,
        &item.id,
        "failed",
        &status,
        Some(agent_id),
        attempts,
        None,
    )?;
    Ok(QueueItemOutcome::failed(status))
}

fn stop_remote_native_disconnected_item(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    state::update_web_queue_item(
        run_id,
        &item.id,
        "stopped",
        REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE,
        Some(agent_id),
        attempts,
        None,
    )?;
    state::update_web_queue_run(
        run_id,
        "stopped",
        item.position,
        REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE,
    )?;
    Ok(QueueItemOutcome::Stopped)
}
