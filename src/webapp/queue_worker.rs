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
            .find(|item| item.status.is_failed_or_blocked())
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
        if reconcile_remote_native_retry(run, item)? {
            changed = true;
            continue;
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
                    "failed".into(),
                    item.position,
                    message.to_string(),
                ));
                continue;
            }
        }
        if item.status.is_success()
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

fn queue_ready_items(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Vec<state::QueueItemRow> {
    if !run.execution_mode.is_graph() {
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
    if queue_item_remote_native(item)
        && item.status.is_starting_or_running()
        && item
            .agent_id
            .as_deref()
            .is_some_and(|agent_id| remote_native_session_running(item, agent_id))
    {
        return false;
    }
    !queue_item_terminal(&item.status)
        && !queue_item_worker_active(&run.id, &item.id)
        && (!item.status.is_starting_or_running()
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
            .is_none_or(|candidate| candidate.status.is_success())
    })
}

fn queue_active_item_count(run_id: &str, items: &[state::QueueItemRow]) -> usize {
    items
        .iter()
        .filter(|item| {
            queue_item_worker_active(run_id, &item.id)
                || item.status.is_active()
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

fn queue_item_terminal(status: impl Into<state::QueueItemStatus>) -> bool {
    status.into().is_terminal()
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
    let mut item = item.clone();
    if let Some(status) = queue_task_status(&item)? {
        if status == "closed:success" {
            update_successful_queue_item(run_id, &item, item.agent_id.as_deref(), item.attempts)?;
            return Ok(QueueItemOutcome::Success);
        }
        if queue_status_auto_recoverable(&status) && queue_item_recovery_active_or_pending(&item) {
            // Keep launching or waiting for the one-shot recovery agent; the old failed task
            // record remains visible until that agent turns it into closed:success.
        } else if queue_status_auto_recoverable(&status) {
            return fail_or_schedule_queue_item_recovery(
                run_id,
                &item,
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
    if item.status.is_starting_or_running() {
        if let Some(agent_id) = item.agent_id.as_deref() {
            if queue_item_remote_native(&item) {
                return wait_for_queue_item_closeout(run_id, &item, agent_id, item.attempts);
            }
            if agent_running(agent_id) {
                if agent_terminal_closeout_failed(agent_id) {
                    let message = "agent reached idle prompt after failed Q-COLD closeout";
                    return fail_or_schedule_queue_item_recovery(
                        run_id,
                        &item,
                        message,
                        Some(agent_id),
                        item.attempts,
                    );
                }
                return wait_for_queue_item_closeout(run_id, &item, agent_id, item.attempts);
            }
            let message = "agent exited before task closeout";
            return fail_or_schedule_queue_item_recovery(
                run_id,
                &item,
                message,
                Some(agent_id),
                item.attempts,
            );
        }
    }
    if item.status.is_stopped_or_paused() {
        if let Some(agent_id) = item.agent_id.as_deref() {
            if queue_item_remote_native(&item) {
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "running",
                    &format!("resumed remote-native agent {agent_id}"),
                    Some(agent_id),
                    item.attempts,
                    None,
                )?;
                let resumed_item = remote_native_running_wait_item(&item);
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
                return wait_for_queue_item_closeout(run_id, &item, agent_id, item.attempts);
            }
        }
    }

    let mut retries = item.attempts.max(0);
    loop {
        if state::web_queue_stop_requested(run_id)? {
            pause_web_queue_item(run_id, &item, None, retries)?;
            return Ok(QueueItemOutcome::Stopped);
        }

        if queue_item_remote_native(&item) {
            state::update_web_queue_item(
                run_id,
                &item.id,
                "starting",
                "starting remote-native agent context",
                None,
                retries,
                None,
            )?;
            let outcome = start_remote_native_queue_item(run_id, &mut item, retries)?;
            if let Some(outcome) =
                handle_queue_launch_outcome(run_id, &mut item, &mut retries, outcome)?
            {
                return Ok(outcome);
            }
            continue;
        }

        if queue_agent_selector_command(&item.agent_command) {
            match select_queue_agent_for_launch(unix_now()) {
                QueueAgentSelection::Selected { command, record } => {
                    if item.agent_command != command {
                        item.agent_command.clone_from(&command);
                        state::set_web_queue_item_agent_command(run_id, &item.id, &command)?;
                    }
                    if record.state != "ok" {
                        let message = format!(
                            "{} is {}: {}",
                            record.command, record.state, record.summary
                        );
                        state::update_web_queue_item(
                            run_id,
                            &item.id,
                            "waiting",
                            &message,
                            None,
                            retries,
                            Some(record.expires_at_unix),
                        )?;
                        let delay = record.expires_at_unix.saturating_sub(unix_now()).max(1);
                        if !sleep_queue_retry(run_id, delay)? {
                            pause_web_queue_item(run_id, &item, None, retries)?;
                            return Ok(QueueItemOutcome::Stopped);
                        }
                        continue;
                    }
                }
                QueueAgentSelection::Waiting {
                    message,
                    next_retry_at,
                } => {
                    state::update_web_queue_item(
                        run_id,
                        &item.id,
                        "waiting",
                        &message,
                        None,
                        retries,
                        Some(next_retry_at),
                    )?;
                    let delay = next_retry_at.saturating_sub(unix_now()).max(1);
                    if !sleep_queue_retry(run_id, delay)? {
                        pause_web_queue_item(run_id, &item, None, retries)?;
                        return Ok(QueueItemOutcome::Stopped);
                    }
                    continue;
                }
            }
        } else if !agents::available_agent_commands()
            .iter()
            .any(|agent| agent.command == item.agent_command)
        {
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
        let outcome = start_web_queue_item(run_id, &item, retries)?;
        if let Some(outcome) = handle_queue_launch_outcome(run_id, &mut item, &mut retries, outcome)? {
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
    state::set_web_queue_item_attempt_terminal(
        run_id,
        &item.id,
        queue_item_semantic_iteration(item),
        &target,
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
            if let Some(outcome) =
                queue_item_status_closeout_outcome(run_id, item, agent_id, attempts, status)?
            {
                return Ok(outcome);
            }
        } else if let Some(outcome) =
            missing_queue_task_record_outcome(run_id, item, agent_id, attempts)?
        {
            return Ok(outcome);
        }
    }
}

fn missing_queue_task_record_outcome(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
) -> Result<Option<QueueItemOutcome>> {
    if queue_item_remote_native(item) {
        if let Some(outcome) = latest_queue_item_terminal_outcome(run_id, &item.id)? {
            return Ok(Some(outcome));
        }
        if remote_native_session_running(item, agent_id) {
            return update_remote_native_missing_record_wait(run_id, item, agent_id, attempts);
        }
        return fail_remote_native_missing_task_record(run_id, item, agent_id, attempts).map(Some);
    }
    if agent_running(agent_id) {
        let _ = submit_agent_terminal_pending_paste(agent_id);
        return Ok(None);
    }
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
    Ok(Some(QueueItemOutcome::retryable_failure(message)))
}

fn update_remote_native_missing_record_wait(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
) -> Result<Option<QueueItemOutcome>> {
    update_queue_item_unless_terminal(
        run_id,
        &item.id,
        "running",
        "waiting for remote-native task record visibility after remote-agent open",
        Some(agent_id),
        attempts,
        None,
    )
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
    match run_remote_agent_contract(item, &repo_root, "down", &session, None, None) {
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

fn continue_resolved_failed_queue_run(run_id: &str) -> Result<bool> {
    let (run, items) = state::load_web_queue_run(run_id)?;
    let Some(run) = run else {
        return Ok(false);
    };
    if run.status.is_active() || run.status.is_success() {
        return Ok(true);
    }
    if !run.status.is_failed() {
        return Ok(false);
    }
    reconcile_one_stale_web_queue_run(run, items)?;
    let (run, _) = state::load_web_queue_run(run_id)?;
    let Some(run) = run else {
        return Ok(false);
    };
    if run.status.is_active() || run.status.is_success() {
        Ok(true)
    } else if run.status.is_failed() {
        bail!(
            "queue is still failed after continue reconciliation: {}",
            run.message
        )
    } else {
        bail!(
            "queue is not resumable after continue reconciliation: {}",
            run.status
        )
    }
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
    if !run.status.is_active() {
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

enum QueueAgentSelection {
    Selected {
        command: String,
        record: Box<AgentLimitRecord>,
    },
    Waiting {
        message: String,
        next_retry_at: u64,
    },
}

fn select_queue_agent_for_launch(now: u64) -> QueueAgentSelection {
    let cache = AGENT_LIMIT_CACHE.get_or_init(|| Mutex::new(None));
    let cached = cache.lock().ok().and_then(|guard| guard.clone());
    let stale = cached
        .as_ref()
        .is_none_or(|cached| now >= cached.generated_at_unix.saturating_add(AGENT_LIMIT_CACHE_TTL));
    if stale {
        schedule_agent_limit_refresh();
    }
    if let Some(cached) = cached {
        return select_queue_agent_from_records(now, &cached.records);
    }
    QueueAgentSelection::Waiting {
        message: "waiting for c1/c2 status probe".to_string(),
        next_retry_at: now.saturating_add(AGENT_LIMIT_PENDING_RETRY),
    }
}

fn select_queue_agent_from_records(now: u64, records: &[AgentLimitRecord]) -> QueueAgentSelection {
    if let Some(record) = records
        .iter()
        .filter(|record| queue_agent_record_usable(record, now))
        .max_by_key(|record| (record.capacity_score, std::cmp::Reverse(record.command.clone())))
    {
        return QueueAgentSelection::Selected {
            command: record.command.clone(),
            record: Box::new(record.clone()),
        };
    }
    let next_retry_at = records
        .iter()
        .filter_map(queue_agent_next_retry_at)
        .filter(|retry_at| *retry_at > now)
        .min()
        .unwrap_or_else(|| now.saturating_add(AGENT_LIMIT_PENDING_RETRY));
    let message = if records.is_empty() {
        "no eligible c1/c2 agent command is available".to_string()
    } else {
        format!("all eligible c1/c2 agents are waiting; {}", agent_limit_summary(records))
    };
    QueueAgentSelection::Waiting {
        message,
        next_retry_at,
    }
}

fn queue_agent_record_usable(record: &AgentLimitRecord, now: u64) -> bool {
    queue_agent_selector_command(&record.command)
        && record.state == "ok"
        && record.capacity_score > 0
        && now < record.expires_at_unix
}

fn queue_agent_next_retry_at(record: &AgentLimitRecord) -> Option<u64> {
    record.reset_at_unix.or(Some(record.expires_at_unix))
}

fn agent_limit_summary(records: &[AgentLimitRecord]) -> String {
    records
        .iter()
        .filter(|record| queue_agent_selector_command(&record.command))
        .map(|record| {
            let retry = record
                .reset_at_unix
                .or(Some(record.expires_at_unix))
                .map(|value| format!(" retry_at={value}"))
                .unwrap_or_default();
            format!(
                "{} state={} capacity={}{} summary={}",
                record.command, record.state, record.capacity_score, retry, record.summary
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn retry_index(retries: i64) -> usize {
    usize::try_from(retries).unwrap_or(usize::MAX)
}
