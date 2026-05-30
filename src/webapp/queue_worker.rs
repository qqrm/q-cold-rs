fn cleanup_existing_task_agent_artifacts(
    task_id: &str,
    task: Option<&state::TaskRecordRow>,
    agent_id: Option<String>,
) -> Result<()> {
    if task.is_some() {
        state::delete_task_record(task_id)?;
    }
    if let Some(agent_id) = agent_id {
        let _ = agents::terminate_agent(&agent_id);
    }
    Ok(())
}

#[cfg(test)]
fn spawn_web_queue_worker(_run_id: String) {}

#[cfg(not(test))]
fn spawn_web_queue_worker(run_id: String) {
    let workers = WEB_QUEUE_WORKERS.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut active) = workers.lock() {
        if !active.insert(run_id.clone()) {
            return;
        }
    }
    thread::spawn(move || {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_web_queue(&run_id))) {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = state::update_web_queue_run(&run_id, "failed", -1, &format!("{err:#}"));
            }
            Err(payload) => {
                let message = format!(
                    "queue worker panicked: {}",
                    panic_payload_message(payload.as_ref())
                );
                let _ = state::update_web_queue_run(&run_id, "failed", -1, &message);
            }
        }
        if let Some(workers) = WEB_QUEUE_WORKERS.get() {
            if let Ok(mut active) = workers.lock() {
                active.remove(&run_id);
            }
        }
    });
}

fn web_queue_worker_active(run_id: &str) -> bool {
    WEB_QUEUE_WORKERS
        .get()
        .and_then(|workers| workers.lock().ok())
        .is_some_and(|active| active.contains(run_id))
}

fn run_web_queue(run_id: &str) -> Result<()> {
    state::update_web_queue_run(run_id, "running", -1, "running")?;
    loop {
        let (run, items) = state::load_web_queue_run(run_id)?;
        let Some(run) = run else {
            return Ok(());
        };
        if items.is_empty() {
            state::update_web_queue_run(run_id, "failed", -1, "queue has no items")?;
            return Ok(());
        }
        crate::sync_codex_task_records().ok();
        match reconcile_queue_task_statuses(&run, &items)? {
            QueueReconcile::Unchanged => {}
            QueueReconcile::Changed => continue,
            QueueReconcile::Terminal => return Ok(()),
        }
        if let Some(item) = items
            .iter()
            .find(|item| matches!(item.status.as_str(), "failed" | "blocked"))
        {
            state::update_web_queue_run(run_id, "failed", item.position, &item.message)?;
            return Ok(());
        }
        if items.iter().all(|item| queue_item_terminal(&item.status)) {
            state::update_web_queue_run(run_id, "success", -1, "closed successfully")?;
            return Ok(());
        }
        if state::web_queue_stop_requested(run_id)? {
            for item in items.iter().filter(|item| !queue_item_terminal(&item.status)) {
                if !queue_item_worker_active(run_id, &item.id) {
                    pause_web_queue_item(run_id, item, item.agent_id.as_deref(), item.attempts)?;
                }
            }
            state::update_web_queue_run(run_id, "stopped", -1, "stopped by operator")?;
            return Ok(());
        }

        let ready = queue_ready_items(&run, &items);
        let mut spawned = 0_usize;
        for item in ready {
            if spawn_web_queue_item_worker(run_id.to_string(), item) {
                spawned += 1;
            }
        }
        let active = queue_active_item_count(run_id, &items);
        let runnable = items
            .iter()
            .filter(|item| !queue_item_terminal(&item.status))
            .count();
        let message = if spawned > 0 {
            format!("started {spawned} ready task(s); {runnable} active or waiting")
        } else if active > 0 {
            format!("running {active} task(s); {runnable} active or waiting")
        } else {
            "waiting for dependencies".to_string()
        };
        state::update_web_queue_run(run_id, "running", -1, &message)?;
        thread::sleep(Duration::from_secs(5));
    }
}

enum QueueReconcile {
    Unchanged,
    Changed,
    Terminal,
}

fn reconcile_queue_task_statuses(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Result<QueueReconcile> {
    let mut changed = false;
    let mut terminal_run: Option<(String, i64, String)> = None;
    for item in items {
        if let Some(status) = queue_task_status(item)? {
            if reconcile_queue_task_record_status(
                run,
                item,
                status,
                &mut changed,
                &mut terminal_run,
            )? {
                continue;
            }
        }
        if let Some(agent_id) = item.agent_id.as_deref() {
            if let Some(message) = queue_agent_failure_message(item, agent_id) {
                if schedule_queue_item_auto_recovery(&run.id, item, message)? {
                    changed = true;
                    continue;
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
                changed = true;
                terminal_run.get_or_insert((
                    "failed".to_string(),
                    item.position,
                    message.to_string(),
                ));
                continue;
            }
        }
        if item.status == "success"
            && item
                .agent_id
                .as_deref()
                .is_some_and(agent_running)
        {
            update_successful_queue_item(&run.id, item, item.agent_id.as_deref(), item.attempts)?;
            changed = true;
        }
    }
    if let Some((status, position, message)) = terminal_run {
        state::update_web_queue_run(&run.id, &status, position, &message)?;
        return Ok(QueueReconcile::Terminal);
    }
    Ok(if changed {
        QueueReconcile::Changed
    } else {
        QueueReconcile::Unchanged
    })
}

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

fn queue_ready_items(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Vec<state::QueueItemRow> {
    if run.execution_mode != "graph" {
        let Some(item) = items
            .iter()
            .filter(|item| !queue_item_terminal(&item.status))
            .min_by_key(|item| (item.position, item.id.as_str()))
        else {
            return Vec::new();
        };
        if queue_item_is_ready_to_spawn(run, item, items) {
            return vec![item.clone()];
        }
        return Vec::new();
    }
    let mut candidates = items
        .iter()
        .filter(|item| queue_item_is_ready_to_spawn(run, item, items))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|item| (item.position, item.id.clone()));
    candidates
}

fn queue_item_is_ready_to_spawn(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    items: &[state::QueueItemRow],
) -> bool {
    !queue_item_terminal(&item.status)
        && !queue_item_worker_active(&run.id, &item.id)
        && (!matches!(item.status.as_str(), "starting" | "running")
            || item
                .agent_id
                .as_deref()
                .is_none_or(|agent_id| !agent_running(agent_id)))
        && item.next_attempt_at.is_none_or(|time| time <= unix_now())
        && queue_dependencies_satisfied(item, items)
}

fn queue_dependencies_satisfied(
    item: &state::QueueItemRow,
    items: &[state::QueueItemRow],
) -> bool {
    item.depends_on.iter().all(|dependency| {
        items
            .iter()
            .find(|candidate| candidate.id == *dependency)
            .is_none_or(|candidate| candidate.status == "success")
    })
}

fn queue_active_item_count(run_id: &str, items: &[state::QueueItemRow]) -> usize {
    items
        .iter()
        .filter(|item| {
            queue_item_worker_active(run_id, &item.id)
                || matches!(item.status.as_str(), "starting" | "running" | "waiting")
        })
        .count()
}

fn queue_item_worker_key(run_id: &str, item_id: &str) -> String {
    format!("{run_id}:{item_id}")
}

fn queue_item_worker_active(run_id: &str, item_id: &str) -> bool {
    let key = queue_item_worker_key(run_id, item_id);
    WEB_QUEUE_ITEM_WORKERS
        .get()
        .and_then(|workers| workers.lock().ok())
        .is_some_and(|active| active.contains(&key))
}

fn spawn_web_queue_item_worker(run_id: String, item: state::QueueItemRow) -> bool {
    let key = queue_item_worker_key(&run_id, &item.id);
    let workers = WEB_QUEUE_ITEM_WORKERS.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut active) = workers.lock() {
        if !active.insert(key.clone()) {
            return false;
        }
    }
    thread::spawn(move || {
        let item_id = item.id.clone();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_web_queue_item(&run_id, &item)
        })) {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                let _ = state::update_web_queue_item(
                    &run_id,
                    &item_id,
                    "failed",
                    &format!("{err:#}"),
                    item.agent_id.as_deref(),
                    item.attempts,
                    None,
                );
            }
            Err(payload) => {
                let message = format!(
                    "queue item worker panicked: {}",
                    panic_payload_message(payload.as_ref())
                );
                let _ = state::update_web_queue_item(
                    &run_id,
                    &item_id,
                    "failed",
                    &message,
                    item.agent_id.as_deref(),
                    item.attempts,
                    None,
                );
            }
        }
        if let Some(workers) = WEB_QUEUE_ITEM_WORKERS.get() {
            if let Ok(mut active) = workers.lock() {
                active.remove(&key);
            }
        }
    });
    true
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_string()
}

fn queue_item_terminal(status: &str) -> bool {
    matches!(status, "success" | "failed" | "blocked")
}

enum QueueItemOutcome {
    Success,
    Stopped,
    RecoveryScheduled,
    Failed { message: String, retryable: bool },
}

impl QueueItemOutcome {
    fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
            retryable: false,
        }
    }

    fn retryable_failure(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
            retryable: true,
        }
    }
}

#[allow(clippy::too_many_lines, reason = "existing queue runner split debt")]
fn run_web_queue_item(run_id: &str, item: &state::QueueItemRow) -> Result<QueueItemOutcome> {
    if let Some(status) = queue_task_status(item)? {
        if status == "closed:success" {
            update_successful_queue_item(run_id, item, item.agent_id.as_deref(), item.attempts)?;
            return Ok(QueueItemOutcome::Success);
        }
        if queue_status_auto_recoverable(&status) && queue_item_recovery_active_or_pending(item) {
            // Keep launching or waiting for the one-shot recovery agent; the old failed task
            // record remains visible until that agent turns it into closed:success.
        } else if queue_status_auto_recoverable(&status) {
            return fail_or_schedule_queue_item_recovery(
                run_id,
                item,
                &status,
                item.agent_id.as_deref(),
                item.attempts,
            );
        } else if queue_task_status_terminal(&status) {
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                &status,
                None,
                item.attempts,
                None,
            )?;
            return Ok(QueueItemOutcome::failed(status));
        }
    }
    if matches!(item.status.as_str(), "running" | "starting") {
        if let Some(agent_id) = item.agent_id.as_deref() {
            if queue_item_remote_native(item) {
                return wait_for_queue_item_closeout(run_id, item, agent_id, item.attempts);
            }
            if agent_running(agent_id) {
                if agent_terminal_closeout_failed(agent_id) {
                    let message = "agent reached idle prompt after failed Q-COLD closeout";
                    return fail_or_schedule_queue_item_recovery(
                        run_id,
                        item,
                        message,
                        Some(agent_id),
                        item.attempts,
                    );
                }
                return wait_for_queue_item_closeout(run_id, item, agent_id, item.attempts);
            }
            let message = "agent exited before task closeout";
            return fail_or_schedule_queue_item_recovery(
                run_id,
                item,
                message,
                Some(agent_id),
                item.attempts,
            );
        }
    }
    if matches!(item.status.as_str(), "stopped" | "paused") {
        if let Some(agent_id) = item.agent_id.as_deref() {
            if queue_item_remote_native(item) {
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "running",
                    &format!("resumed remote-native agent {agent_id}"),
                    Some(agent_id),
                    item.attempts,
                    None,
                )?;
                let resumed_item = remote_native_running_wait_item(item);
                return wait_for_queue_item_closeout(run_id, &resumed_item, agent_id, item.attempts);
            }
            if agent_running(agent_id) {
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "running",
                    &format!("resumed agent {agent_id}"),
                    Some(agent_id),
                    item.attempts,
                    None,
                )?;
                return wait_for_queue_item_closeout(run_id, item, agent_id, item.attempts);
            }
        }
    }

    let mut retries = item.attempts.max(0);
    loop {
        if state::web_queue_stop_requested(run_id)? {
            pause_web_queue_item(run_id, item, None, retries)?;
            return Ok(QueueItemOutcome::Stopped);
        }

        if queue_item_remote_native(item) {
            state::update_web_queue_item(
                run_id,
                &item.id,
                "starting",
                "starting remote-native agent context",
                None,
                retries,
                None,
            )?;
            let outcome = start_remote_native_queue_item(run_id, item, retries)?;
            if let Some(outcome) =
                handle_queue_launch_outcome(run_id, item, &mut retries, outcome)?
            {
                return Ok(outcome);
            }
            continue;
        }

        if let Some(limit) = queue_agent_limit_for_command(&item.agent_command) {
            if limit.state != "ok" {
                if limit.state == "unauthenticated"
                    || retry_index(retries) >= WEB_QUEUE_RETRY_DELAYS.len()
                {
                    let message = format!(
                        "{} is {}: {}",
                        item.agent_command, limit.state, limit.summary
                    );
                    state::update_web_queue_item(
                        run_id,
                        &item.id,
                        "failed",
                        &message,
                        None,
                        retries,
                        None,
                    )?;
                    return Ok(QueueItemOutcome::failed(message));
                }
                let delay = WEB_QUEUE_RETRY_DELAYS[retry_index(retries)];
                retries += 1;
                let next_attempt_at = unix_now().saturating_add(delay);
                let message = format!(
                    "{} is {}: {}; retry {}/{} in {}s",
                    item.agent_command,
                    limit.state,
                    limit.summary,
                    retries,
                    WEB_QUEUE_RETRY_DELAYS.len(),
                    delay
                );
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "waiting",
                    &message,
                    None,
                    retries,
                    Some(next_attempt_at),
                )?;
                if !sleep_queue_retry(run_id, delay)? {
                    pause_web_queue_item(run_id, item, None, retries)?;
                    return Ok(QueueItemOutcome::Stopped);
                }
                continue;
            }
        } else {
            let message = format!("unknown queue agent command: {}", item.agent_command);
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                &message,
                None,
                retries,
                None,
            )?;
            return Ok(QueueItemOutcome::failed(message));
        }

        state::update_web_queue_item(
            run_id,
            &item.id,
            "starting",
            "starting clean agent context",
            None,
            retries,
            None,
        )?;
        let outcome = start_web_queue_item(run_id, item, retries)?;
        if let Some(outcome) = handle_queue_launch_outcome(run_id, item, &mut retries, outcome)? {
            return Ok(outcome);
        }
    }
}

fn start_web_queue_item(
    run_id: &str,
    item: &state::QueueItemRow,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    let task = match queue_launch_workspace(item) {
        Ok(task) => task,
        Err(err) => {
            let message = format!("{err:#}");
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                &message,
                None,
                attempts,
                None,
            )?;
            return Ok(QueueItemOutcome::failed(message));
        }
    };
    if let Err(err) = cleanup_stale_queue_agent_launch_artifacts(item, &task.worktree) {
        let message = format!("{err:#}");
        state::update_web_queue_item(
            run_id,
            &item.id,
            "failed",
            &message,
            Some(&queue_agent_id(item)),
            attempts,
            None,
        )?;
        return Ok(QueueItemOutcome::failed(message));
    }
    let prompt_file = match write_queue_task_packet_file(item, &task) {
        Ok(path) => path,
        Err(err) => {
            let message = format!("{err:#}");
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                &message,
                None,
                attempts,
                None,
            )?;
            return Ok(QueueItemOutcome::failed(message));
        }
    };
    let command = queue_agent_launch_command(item, &task, &prompt_file);
    let request = AgentStartRequest {
        id: Some(queue_agent_id(item)),
        cwd: Some(task.worktree),
        track: queue_track(run_id),
        command,
    };
    let agent = match start_web_agent(&request) {
        Ok(agent) => agent,
        Err(err) => {
            cleanup_queue_task_packet_file(&prompt_file);
            return Ok(QueueItemOutcome::retryable_failure(format!("{err:#}")));
        }
    };
    state::update_web_queue_item(
        run_id,
        &item.id,
        "starting",
        "waiting for agent terminal",
        Some(&agent.id),
        attempts,
        None,
    )?;
    remember_queue_task_agent(item, &agent.id)?;
    let Some(target) = wait_for_agent_terminal_target(&agent.id) else {
        cleanup_queue_task_packet_file(&prompt_file);
        return Ok(retry_after_queue_agent_launch_failure(
            &agent.id,
            "agent terminal did not appear",
        ));
    };
    set_queue_terminal_scope(&target, item)?;
    state::update_web_queue_item(
        run_id,
        &item.id,
        "running",
        &format!("{} ({})", queue_display_label(item), agent.id),
        Some(&agent.id),
        attempts,
        None,
    )?;
    let outcome = wait_for_queue_item_closeout(run_id, item, &agent.id, attempts);
    cleanup_queue_task_packet_file(&prompt_file);
    outcome
}

fn set_queue_terminal_scope(target: &str, item: &state::QueueItemRow) -> Result<()> {
    let scope = queue_terminal_scope(item);
    let name = queue_display_label(item);
    state::save_terminal_metadata(target, Some(&name), Some(&scope))
}

fn queue_terminal_scope(item: &state::QueueItemRow) -> String {
    format!("task/{}", item.slug)
}

fn queue_display_label(item: &state::QueueItemRow) -> String {
    item.repo_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map_or_else(|| item.slug.clone(), |name| format!("{name} {}", item.slug))
}

fn wait_for_queue_item_closeout(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    loop {
        if state::web_queue_stop_requested(run_id)? {
            pause_web_queue_item(run_id, item, Some(agent_id), attempts)?;
            return Ok(QueueItemOutcome::Stopped);
        }
        thread::sleep(Duration::from_secs(5));
        crate::sync_codex_task_records().ok();
        if let Some(status) = queue_task_status(item)? {
            if status == "closed:success" {
                update_successful_queue_item(run_id, item, Some(agent_id), attempts)?;
                return Ok(QueueItemOutcome::Success);
            }
            if status == "paused" {
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "paused",
                    &status,
                    Some(agent_id),
                    attempts,
                    None,
                )?;
                state::update_web_queue_run(run_id, "stopped", item.position, &status)?;
                return Ok(QueueItemOutcome::Stopped);
            }
            if queue_status_auto_recoverable(&status)
                && queue_item_recovery_active_or_pending(item)
                && agent_running(agent_id)
                && !agent_terminal_closeout_failed(agent_id)
            {
                continue;
            }
            if queue_status_auto_recoverable(&status) {
                return fail_or_schedule_queue_item_recovery(
                    run_id,
                    item,
                    &status,
                    Some(agent_id),
                    attempts,
                );
            }
            if queue_task_status_terminal(&status) {
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "failed",
                    &status,
                    Some(agent_id),
                    attempts,
                    None,
                )?;
                return Ok(QueueItemOutcome::failed(status));
            }
            if status == "open" && !queue_item_remote_native(item) && !agent_running(agent_id) {
                let message = "agent exited before task closeout".to_string();
                return fail_or_schedule_queue_item_recovery(
                    run_id,
                    item,
                    &message,
                    Some(agent_id),
                    attempts,
                );
            }
            if status == "open"
                && !queue_item_remote_native(item)
                && submit_agent_terminal_pending_paste(agent_id).unwrap_or(false)
            {
                continue;
            }
            if status == "open"
                && !queue_item_remote_native(item)
                && agent_terminal_closeout_failed(agent_id)
            {
                let message = "agent reached idle prompt after failed Q-COLD closeout".to_string();
                return fail_or_schedule_queue_item_recovery(
                    run_id,
                    item,
                    &message,
                    Some(agent_id),
                    attempts,
                );
            }
        } else if queue_item_remote_native(item) {
            return fail_remote_native_missing_task_record(run_id, item, agent_id, attempts);
        } else if !agent_running(agent_id) {
            let message = "agent exited before opening task record".to_string();
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                &message,
                Some(agent_id),
                attempts,
                None,
            )?;
            return Ok(QueueItemOutcome::retryable_failure(message));
        } else {
            let _ = submit_agent_terminal_pending_paste(agent_id);
        }
    }
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
    match run_remote_agent_contract(item, &repo_root, "down", &session, None) {
        Ok(()) => "remote-agent session stopped".to_string(),
        Err(err) => format!("remote-agent cleanup failed: {err:#}"),
    }
}

fn reconcile_stale_web_queue_run() -> Result<()> {
    for (run, items) in state::load_web_queue_runs()? {
        reconcile_one_stale_web_queue_run(run, items)?;
    }
    Ok(())
}

fn reconcile_one_stale_web_queue_run(
    mut run: state::QueueRunRow,
    mut items: Vec<state::QueueItemRow>,
) -> Result<()> {
    if web_queue_worker_active(&run.id) {
        return Ok(());
    }
    if !queue_run_needs_stale_reconcile(&run, &items)? {
        return Ok(());
    }

    crate::sync_codex_task_records().ok();
    cleanup_orphaned_queue_agents(&run, &items);
    if let QueueReconcile::Changed | QueueReconcile::Terminal =
        reconcile_queue_task_statuses(&run, &items)?
    {
        let (updated_run, updated_items) = state::load_web_queue_run(&run.id)?;
        let Some(updated_run) = updated_run else {
            return Ok(());
        };
        run = updated_run;
        items = updated_items;
    }
    if let Some((updated_run, updated_items)) = restart_resolved_failed_queue_run(&run, &items)? {
        return resume_stale_active_queue_run(&updated_run, updated_items);
    }
    if !matches!(
        run.status.as_str(),
        "running" | "waiting" | "starting" | "stopping"
    ) {
        return Ok(());
    }
    resume_stale_active_queue_run(&run, items)
}

fn cleanup_orphaned_queue_agents(run: &state::QueueRunRow, items: &[state::QueueItemRow]) {
    let known_agents = items
        .iter()
        .filter_map(|item| item.agent_id.as_deref())
        .collect::<HashSet<_>>();
    let track = queue_track(&run.id);
    let Ok(contexts) = agents::terminal_contexts() else {
        return;
    };
    for context in contexts {
        if context.track == track && !known_agents.contains(context.id.as_str()) {
            let _ = agents::terminate_agent(&context.id);
        }
    }
}

fn queue_agent_limit_for_command(command: &str) -> Option<AgentLimitRecord> {
    let agent = agents::available_agent_commands()
        .into_iter()
        .find(|agent| agent.command == command)?;
    Some(probe_agent_limit(&agent))
}

fn retry_index(retries: i64) -> usize {
    usize::try_from(retries).unwrap_or(usize::MAX)
}

fn queue_task_status(item: &state::QueueItemRow) -> Result<Option<String>> {
    let task_id = format!("task/{}", item.slug);
    if item.remote_launcher.is_some() {
        let required_remote_native_sync = remote_native_requires_task_record_sync(item);
        let sync_result = sync_remote_queue_task_records(item, required_remote_native_sync);
        if let Err(err) = sync_result {
            if required_remote_native_sync {
                if let Some(status) = remote_native_sync_failure_fallback_status(item, &task_id)? {
                    return Ok(status);
                }
                return Err(err).context("remote-native task-record sync failed");
            }
        }
    }
    let Some(record) = state::get_task_record(&task_id)? else {
        return Ok(None);
    };
    if !queue_task_record_matches_item(item, &record) {
        return Ok(None);
    }
    Ok(Some(record.status))
}

fn remote_native_sync_failure_fallback_status(
    item: &state::QueueItemRow,
    task_id: &str,
) -> Result<Option<Option<String>>> {
    let Some(record) = state::get_task_record(task_id)? else {
        return Ok(None);
    };
    if !queue_task_record_matches_item(item, &record) {
        return Ok(None);
    }
    if !queue_task_record_is_terminal(&record) {
        return Ok(Some(Some(record.status)));
    }
    if queue_status_auto_recoverable(&record.status) && queue_item_recovery_active_or_pending(item)
    {
        return Ok(Some(None));
    }
    Ok(None)
}

fn remote_native_requires_task_record_sync(item: &state::QueueItemRow) -> bool {
    queue_item_remote_native(item) && matches!(item.status.as_str(), "starting" | "running")
}

fn agent_running(agent_id: &str) -> bool {
    agents::running_snapshot()
        .is_ok_and(|snapshot| snapshot.contains(&format!("agent\t{agent_id}\t")))
}

fn wait_for_agent_terminal_target(agent_id: &str) -> Option<String> {
    for _ in 0..20 {
        if let Some(target) = agents::terminal_contexts()
            .ok()?
            .into_iter()
            .find(|context| context.id == agent_id)
            .map(|context| context.target)
        {
            return Some(target);
        }
        thread::sleep(Duration::from_millis(500));
    }
    None
}

fn sleep_queue_retry(run_id: &str, delay_seconds: u64) -> Result<bool> {
    let mut slept = 0;
    while slept < delay_seconds {
        if state::web_queue_stop_requested(run_id)? {
            return Ok(false);
        }
        let step = (delay_seconds - slept).min(5);
        thread::sleep(Duration::from_secs(step));
        slept += step;
    }
    Ok(true)
}
