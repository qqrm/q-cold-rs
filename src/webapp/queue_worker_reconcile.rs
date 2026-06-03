fn reconcile_stale_web_queue_run_soon() {
    let worker = WEB_QUEUE_RECONCILE_WORKER.get_or_init(|| Mutex::new(false));
    if let Ok(mut active) = worker.lock() {
        if *active {
            return;
        }
        *active = true;
    } else {
        return;
    }
    thread::spawn(|| {
        if let Err(err) = reconcile_stale_web_queue_run() {
            eprintln!("warning: failed to reconcile stale web queue run: {err:#}");
        }
        if let Some(worker) = WEB_QUEUE_RECONCILE_WORKER.get() {
            if let Ok(mut active) = worker.lock() {
                *active = false;
            }
        }
    });
}

fn queue_run_needs_stale_reconcile(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Result<bool> {
    if matches!(
        run.status.as_str(),
        "running" | "waiting" | "starting" | "stopping"
    ) || items.iter().any(|item| {
        matches!(item.status.as_str(), "running" | "starting")
            || (item.status == "success"
                && item
                    .agent_id
                    .as_deref()
                    .is_some_and(agent_running))
    }) {
        return Ok(true);
    }
    failed_queue_run_may_be_resolved(run, items)
}

fn failed_queue_run_may_be_resolved(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Result<bool> {
    if run.status != "failed" || items.is_empty() {
        return Ok(false);
    }
    if !items
        .iter()
        .any(|item| matches!(item.status.as_str(), "failed" | "blocked"))
    {
        return Ok(true);
    }
    for item in items {
        if !matches!(item.status.as_str(), "failed" | "blocked") {
            continue;
        }
        if remote_native_retry_session_running(item) {
            return Ok(true);
        }
        if let Some(status) = queue_task_status(item)? {
            if status == "closed:success"
                || (queue_status_auto_recoverable(&status)
                    && item.recovery_attempts < WEB_QUEUE_AUTO_RECOVERY_ATTEMPTS)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn queue_agent_failure_message(item: &state::QueueItemRow, agent_id: &str) -> Option<&'static str> {
    if !matches!(item.status.as_str(), "running" | "starting") {
        return None;
    }
    if queue_item_remote_native(item) {
        return None;
    }
    if !agent_running(agent_id) {
        return Some("agent exited before task closeout");
    }
    if agent_terminal_closeout_failed(agent_id) {
        return Some("agent reached idle prompt after failed Q-COLD closeout");
    }
    None
}

const REMOTE_NATIVE_RETRY_RUNNING_MESSAGE: &str =
    "remote-native retry is still running after failed closeout";
const REMOTE_NATIVE_MISSING_RECORD_RELAUNCH_MESSAGE: &str =
    "remote-native task record and session are missing; relaunching item";

fn reconcile_remote_native_retry(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
) -> Result<bool> {
    if !matches!(item.status.as_str(), "failed" | "blocked") {
        return Ok(false);
    }
    let Some(agent_id) = item.agent_id.as_deref() else {
        return Ok(false);
    };
    if !remote_native_retry_session_running(item) {
        return Ok(false);
    }
    state::update_web_queue_item(
        &run.id,
        &item.id,
        "running",
        REMOTE_NATIVE_RETRY_RUNNING_MESSAGE,
        Some(agent_id),
        item.attempts,
        None,
    )?;
    Ok(true)
}

fn reconcile_remote_native_missing_record_launch(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
) -> Result<bool> {
    if !matches!(item.status.as_str(), "starting" | "running") || !queue_item_remote_native(item) {
        return Ok(false);
    }
    let Some(agent_id) = item.agent_id.as_deref() else {
        return Ok(false);
    };
    if remote_native_session_running(item, agent_id) {
        return Ok(false);
    }
    if state::get_task_record(&format!("task/{}", item.slug))?.is_some() {
        return Ok(false);
    }
    state::reset_web_queue_item_for_relaunch(
        &run.id,
        &item.id,
        REMOTE_NATIVE_MISSING_RECORD_RELAUNCH_MESSAGE,
        item.attempts,
    )?;
    state::update_web_queue_run(
        &run.id,
        "running",
        item.position,
        REMOTE_NATIVE_MISSING_RECORD_RELAUNCH_MESSAGE,
    )?;
    Ok(true)
}

fn fail_queue_run_item(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    message: &str,
) -> Result<()> {
    state::update_web_queue_item(
        run_id,
        &item.id,
        "failed",
        message,
        Some(agent_id),
        item.attempts,
        None,
    )?;
    state::update_web_queue_run(run_id, "failed", item.position, message)?;
    Ok(())
}

fn resume_stale_active_queue_run(
    run: &state::QueueRunRow,
    items: Vec<state::QueueItemRow>,
) -> Result<()> {
    for item in items {
        if reconcile_remote_native_missing_record_launch(run, &item)? {
            spawn_web_queue_worker(run.id.clone());
            return Ok(());
        }
        if stale_queue_task_record_handled(run, &item)? {
            continue;
        }
        if item.status == "success" {
            close_running_success_agent(run, &item)?;
            continue;
        }
        if reconcile_remote_native_retry(run, &item)? {
            state::update_web_queue_run(
                &run.id,
                "running",
                item.position,
                REMOTE_NATIVE_RETRY_RUNNING_MESSAGE,
            )?;
            spawn_web_queue_worker(run.id.clone());
            return Ok(());
        }
        if matches!(item.status.as_str(), "failed" | "blocked") {
            state::update_web_queue_run(&run.id, "failed", item.position, &item.message)?;
            return Ok(());
        }
        if let Some(agent_id) = item.agent_id.as_deref() {
            if let Some(message) = queue_agent_failure_message(&item, agent_id) {
                fail_queue_run_item(&run.id, &item, agent_id, message)?;
                return Ok(());
            }
        }
        state::update_web_queue_run(
            &run.id,
            "running",
            item.position,
            &format!("running {}", item.slug),
        )?;
        spawn_web_queue_worker(run.id.clone());
        return Ok(());
    }

    state::update_web_queue_run(&run.id, "success", -1, "closed successfully")?;
    Ok(())
}

fn restart_resolved_failed_queue_run(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Result<Option<(state::QueueRunRow, Vec<state::QueueItemRow>)>> {
    if run.status != "failed" {
        return Ok(None);
    }
    if items
        .iter()
        .any(|item| matches!(item.status.as_str(), "failed" | "blocked"))
    {
        return Ok(None);
    }
    if items.iter().all(|item| item.status == "success") {
        state::update_web_queue_run(&run.id, "success", -1, "closed successfully")?;
        return Ok(None);
    }
    state::update_web_queue_run(
        &run.id,
        "running",
        -1,
        "resuming after resolved blocked task",
    )?;
    let (updated_run, updated_items) = state::load_web_queue_run(&run.id)?;
    let Some(updated_run) = updated_run else {
        return Ok(None);
    };
    Ok(Some((updated_run, updated_items)))
}

fn stale_queue_task_record_handled(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
) -> Result<bool> {
    let Some(status) = queue_task_status(item)? else {
        return Ok(false);
    };
    if status == "closed:success" {
        if item.status != "success" || item.agent_id.as_deref().is_some_and(agent_running) {
            update_successful_queue_item(&run.id, item, item.agent_id.as_deref(), item.attempts)?;
        }
        return Ok(true);
    }
    if queue_status_auto_recoverable(&status)
        && queue_item_recovery_waiting_on_current_attempt(item)
    {
        return Ok(false);
    }
    if queue_status_auto_recoverable(&status)
        && schedule_queue_item_auto_recovery(&run.id, item, &status)?
    {
        return Ok(true);
    }
    if status == "paused" {
        state::update_web_queue_item(
            &run.id,
            &item.id,
            "paused",
            &status,
            item.agent_id.as_deref(),
            item.attempts,
            None,
        )?;
        state::update_web_queue_run(&run.id, "stopped", item.position, &status)?;
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
        state::update_web_queue_run(&run.id, "failed", item.position, &status)?;
        return Ok(true);
    }
    Ok(false)
}

fn close_running_success_agent(run: &state::QueueRunRow, item: &state::QueueItemRow) -> Result<()> {
    if item.agent_id.as_deref().is_some_and(agent_running) {
        update_successful_queue_item(&run.id, item, item.agent_id.as_deref(), item.attempts)?;
    }
    Ok(())
}
