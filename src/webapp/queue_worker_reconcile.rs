fn queue_run_needs_stale_reconcile(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> bool {
    matches!(
        run.status.as_str(),
        "running" | "waiting" | "starting" | "stopping"
    ) || items.iter().any(|item| {
        matches!(item.status.as_str(), "running" | "starting")
            || (item.status == "success"
                && item
                    .agent_id
                    .as_deref()
                    .is_some_and(agent_running))
    })
}

fn queue_agent_failure_message(item: &state::QueueItemRow, agent_id: &str) -> Option<&'static str> {
    if !matches!(item.status.as_str(), "running" | "starting") {
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
        if stale_queue_task_record_handled(run, &item)? {
            continue;
        }
        if item.status == "success" {
            close_running_success_agent(run, &item)?;
            continue;
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
    if status.starts_with("closed") && item.status != "success" {
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
