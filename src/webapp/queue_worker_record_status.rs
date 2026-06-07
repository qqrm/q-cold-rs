const REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE: &str =
    "remote-native task is open, but remote-agent session is not running on the remote host";
const REMOTE_NATIVE_OPEN_RECORD_RELAUNCH_MESSAGE: &str =
    "remote-native task is open but remote-agent session is missing; relaunching item";
const LOCAL_OPEN_RECORD_STOPPED_MESSAGE: &str =
    "local task is open but agent session is missing; press Continue to resume";

fn reconcile_queue_task_record_status(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    status: String,
    changed: &mut bool,
    terminal_run: &mut Option<(String, i64, String)>,
) -> Result<bool> {
    if status == "closed:success" {
        if !item.status.is_success() || item.agent_id.as_deref().is_some_and(agent_running) {
            update_successful_queue_item(&run.id, item, item.agent_id.as_deref(), item.attempts)?;
            *changed = true;
        }
        return Ok(true);
    }
    if reconcile_remote_native_open_record(run, item, &status, changed)? {
        return Ok(true);
    }
    if local_open_record_without_live_agent(item, &status) {
        mark_local_open_queue_item_stopped(
            &run.id,
            item,
            item.agent_id.as_deref(),
            item.attempts,
        )?;
        *changed = true;
        terminal_run.get_or_insert((
            "stopped".into(),
            item.position,
            LOCAL_OPEN_RECORD_STOPPED_MESSAGE.to_string(),
        ));
        return Ok(true);
    }
    if status == "paused" && !item.status.is_paused() {
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
        terminal_run.get_or_insert(("stopped".into(), item.position, status));
        return Ok(true);
    }
    if queue_status_auto_recoverable(&status) {
        if let Some(agent_id) = live_queue_item_recovery_agent_id(item) {
            let message = format!("running recovery retry ({agent_id})");
            if !item.status.is_running()
                || item.message != message
                || item.agent_id.as_deref() != Some(agent_id)
            {
                state::update_web_queue_item(
                    &run.id,
                    &item.id,
                    "running",
                    &message,
                    Some(agent_id),
                    item.attempts,
                    None,
                )?;
                *changed = true;
            }
            state::update_web_queue_run(&run.id, "running", item.position, &message)?;
            return Ok(true);
        }
        if queue_item_recovery_waiting_on_current_attempt(item) {
            return Ok(true);
        }
    }
    if queue_status_auto_recoverable(&status)
        && schedule_queue_item_auto_recovery(&run.id, item, &status)?
    {
        *changed = true;
        return Ok(true);
    }
    if queue_task_status_terminal(&status) && !item.status.is_success() {
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
        terminal_run.get_or_insert(("failed".into(), item.position, status));
        return Ok(true);
    }
    Ok(false)
}

fn local_open_record_without_live_agent(item: &state::QueueItemRow, status: &str) -> bool {
    status == "open"
        && !queue_item_remote_native(item)
        && !item.status.is_success()
        && item
            .agent_id
            .as_deref()
            .is_none_or(|agent_id| !agent_running(agent_id))
}

fn reconcile_remote_native_open_record(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    status: &str,
    changed: &mut bool,
) -> Result<bool> {
    if let Some(agent_id) = remote_native_stopped_open_record_live_agent_id(item, status) {
        let message = format!("resumed remote-native agent {agent_id}");
        state::update_web_queue_item(
            &run.id,
            &item.id,
            "running",
            &message,
            Some(&agent_id),
            item.attempts,
            None,
        )?;
        state::update_web_queue_run(&run.id, "running", item.position, "running")?;
        *changed = true;
        return Ok(true);
    }
    if remote_native_stopped_disconnected_open_record_without_live_session(item, status) {
        relaunch_remote_native_disconnected_item(&run.id, item, item.attempts)?;
        *changed = true;
        return Ok(true);
    }
    if let Some(agent_id) = remote_native_active_open_record_live_agent_id(item, status) {
        let message = remote_native_active_open_message(item, &agent_id);
        if !item.status.is_running()
            || item.message != message
            || item.agent_id.as_deref() != Some(agent_id.as_str())
        {
            state::update_web_queue_item(
                &run.id,
                &item.id,
                "running",
                &message,
                Some(&agent_id),
                item.attempts,
                None,
            )?;
            *changed = true;
        }
        return Ok(true);
    }
    if remote_native_open_record_without_live_session(item, status) {
        relaunch_remote_native_disconnected_item(&run.id, item, item.attempts)?;
        *changed = true;
        return Ok(true);
    }
    Ok(false)
}

fn remote_native_active_open_message(item: &state::QueueItemRow, agent_id: &str) -> String {
    format!("{} ({agent_id})", queue_display_label(item))
}

fn remote_native_active_open_record_live_agent_id(
    item: &state::QueueItemRow,
    status: &str,
) -> Option<String> {
    (status == "open"
        && queue_item_remote_native(item)
        && item.status.is_starting_or_running())
    .then(|| remote_native_open_record_live_agent_id(item))
    .flatten()
}

fn remote_native_open_record_without_live_session(
    item: &state::QueueItemRow,
    status: &str,
) -> bool {
    status == "open"
        && queue_item_remote_native(item)
        && remote_native_open_record_needs_live_session(item)
        && remote_native_open_record_live_agent_id(item).is_none()
}

fn remote_native_open_record_needs_live_session(item: &state::QueueItemRow) -> bool {
    matches!(
        item.status,
        state::QueueItemStatus::Starting
            | state::QueueItemStatus::Running
            | state::QueueItemStatus::Waiting
    )
}

fn remote_native_stopped_open_record_live_agent_id(
    item: &state::QueueItemRow,
    status: &str,
) -> Option<String> {
    (status == "open"
        && queue_item_remote_native(item)
        && item.status.is_stopped_or_paused())
    .then(|| remote_native_open_record_live_agent_id(item))
    .flatten()
}

fn remote_native_stopped_disconnected_open_record_without_live_session(
    item: &state::QueueItemRow,
    status: &str,
) -> bool {
    status == "open"
        && queue_item_remote_native(item)
        && item.status.is_stopped_or_paused()
        && item.message == REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE
        && remote_native_open_record_live_agent_id(item).is_none()
}

fn remote_native_open_record_live_agent_id(item: &state::QueueItemRow) -> Option<String> {
    let agent_id = item.agent_id.as_deref()?.trim();
    if agent_id.is_empty() {
        return None;
    }
    if remote_native_session_running(item, agent_id) {
        return Some(agent_id.to_string());
    }
    remote_native_retry_agent_ids(agent_id)
        .into_iter()
        .find(|candidate| remote_native_session_running(item, candidate))
}

fn remote_native_retry_agent_ids(agent_id: &str) -> Vec<String> {
    let Some((prefix, _suffix)) = agent_id.rsplit_once('-') else {
        return Vec::new();
    };
    if prefix.is_empty() {
        return Vec::new();
    }
    ["relaunch", "repair"]
        .into_iter()
        .flat_map(|kind| {
            ["", "2"]
                .into_iter()
                .map(move |ordinal| format!("{prefix}-{kind}{ordinal}"))
        })
        .filter(|candidate| candidate != agent_id)
        .collect()
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
    if let Some(live_agent_id) = remote_native_stopped_open_record_live_agent_id(item, &status) {
        state::update_web_queue_item(
            run_id,
            &item.id,
            "running",
            &format!("resumed remote-native agent {live_agent_id}"),
            Some(&live_agent_id),
            attempts,
            None,
        )?;
        state::update_web_queue_run(run_id, "running", item.position, "running")?;
        return Ok(None);
    }
    if remote_native_open_record_without_live_session(item, &status) {
        return relaunch_remote_native_disconnected_item(run_id, item, attempts).map(Some);
    }
    if status == "open" && !queue_item_remote_native(item) && !agent_running(agent_id) {
        return mark_local_open_queue_item_stopped(run_id, item, Some(agent_id), attempts)
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

fn mark_local_open_queue_item_stopped(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: Option<&str>,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    state::update_web_queue_item(
        run_id,
        &item.id,
        "stopped",
        LOCAL_OPEN_RECORD_STOPPED_MESSAGE,
        agent_id,
        attempts,
        None,
    )?;
    state::update_web_queue_run(
        run_id,
        "stopped",
        item.position,
        LOCAL_OPEN_RECORD_STOPPED_MESSAGE,
    )?;
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

fn relaunch_remote_native_disconnected_item(
    run_id: &str,
    item: &state::QueueItemRow,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    state::reset_web_queue_item_for_relaunch(
        run_id,
        &item.id,
        REMOTE_NATIVE_OPEN_RECORD_RELAUNCH_MESSAGE,
        attempts,
    )?;
    state::update_web_queue_run(
        run_id,
        "running",
        item.position,
        REMOTE_NATIVE_OPEN_RECORD_RELAUNCH_MESSAGE,
    )?;
    Ok(QueueItemOutcome::RecoveryScheduled)
}
