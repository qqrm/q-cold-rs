const QUEUE_LOCAL_LAUNCH_FAILED_BEFORE_TASK_RECORD_RELAUNCH_MESSAGE: &str =
    "local executor exited before opening task record; relaunching item";

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
fn spawn_web_queue_worker(run_id: String) {
    let spawns = TEST_WEB_QUEUE_WORKER_SPAWNS.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut spawns) = spawns.lock() {
        spawns.push(run_id);
    }
}

#[cfg(test)]
fn test_web_queue_worker_spawned(run_id: &str) -> bool {
    TEST_WEB_QUEUE_WORKER_SPAWNS
        .get()
        .and_then(|spawns| spawns.lock().ok())
        .is_some_and(|spawns| spawns.iter().any(|spawn| spawn == run_id))
}

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
        state::recover_stale_web_queue_item_worker_leases(run_id)?;
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
            let activity = queue_activity_snapshot(run_id);
            for item in items.iter().filter(|item| !queue_item_terminal(&item.status)) {
                if !queue_item_worker_active_in_snapshot(&item.id, &activity) {
                    pause_web_queue_item(run_id, item, item.agent_id.as_deref(), item.attempts)?;
                }
            }
            state::update_web_queue_run(run_id, "stopped", -1, "stopped by operator")?;
            return Ok(());
        }

        let activity = queue_activity_snapshot(run_id);
        let QueueAdmissionPlan { admitted, waiting } =
            apply_queue_admission(queue_ready_items_with_activity(&run, &items, &activity))?;
        let mut spawned = 0_usize;
        for item in admitted {
            if spawn_web_queue_item_worker(run_id.to_string(), item) {
                spawned += 1;
            }
        }
        let active = queue_active_item_count_with_activity(&items, &activity);
        let runnable = items
            .iter()
            .filter(|item| !queue_item_terminal(&item.status))
            .count();
        let message = if spawned > 0 {
            format!("started {spawned} ready task(s); {runnable} active or waiting")
        } else if let Some((_, wait)) = waiting.first() {
            format!(
                "waiting for resource admission: {}; retry_at={}",
                wait.reason, wait.next_retry_at
            )
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
        let evidence = collect_queue_status_evidence_for_item(run, item)?;
        let reduction = reduce_queue_status(&evidence);
        if let Some(status) = evidence.task_status.as_deref() {
            if execute_queue_status_reduction(
                run,
                item,
                status,
                &evidence,
                &reduction,
                &mut changed,
                &mut terminal_run,
            )? {
                continue;
            }
        } else if let Some(item_changed) =
            reconcile_queue_item_without_task_record(run, item, &reduction, &mut terminal_run)?
        {
            changed |= item_changed;
            continue;
        }
        if reconcile_remote_native_retry(run, item)? {
            changed = true;
            continue;
        }
        if reduction.handled {
            continue;
        }
        if reconcile_queue_item_fallback_status(run, item, &mut terminal_run)? {
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

#[cfg(test)]
fn queue_ready_items(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Vec<state::QueueItemRow> {
    let activity = queue_activity_snapshot(&run.id);
    queue_ready_items_with_activity(run, items, &activity)
}

fn queue_ready_items_with_activity(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
    activity: &QueueActivitySnapshot,
) -> Vec<state::QueueItemRow> {
    if !run.execution_mode.is_graph() {
        let Some(item) = items
            .iter()
            .filter(|item| !queue_item_terminal(&item.status))
            .min_by_key(|item| (item.position, item.id.as_str()))
        else {
            return Vec::new();
        };
        if queue_item_is_ready_to_spawn(item, items, activity) {
            return vec![item.clone()];
        }
        return Vec::new();
    }
    let mut candidates = items
        .iter()
        .filter(|item| queue_item_is_ready_to_spawn(item, items, activity))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|item| (item.position, item.id.clone()));
    candidates
}

fn queue_item_is_ready_to_spawn(
    item: &state::QueueItemRow,
    items: &[state::QueueItemRow],
    activity: &QueueActivitySnapshot,
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
        && !queue_item_worker_active_in_snapshot(&item.id, activity)
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

fn queue_active_item_count_with_activity(
    items: &[state::QueueItemRow],
    activity: &QueueActivitySnapshot,
) -> usize {
    items
        .iter()
        .filter(|item| {
            queue_item_worker_active_in_snapshot(&item.id, activity)
                || item.status.is_active()
        })
        .count()
}

fn queue_item_worker_key(run_id: &str, item_id: &str) -> String {
    format!("{run_id}:{item_id}")
}

fn queue_item_worker_active(run_id: &str, item_id: &str) -> bool {
    let activity = queue_activity_snapshot(run_id);
    queue_item_worker_active_in_snapshot(item_id, &activity)
}

fn spawn_web_queue_item_worker(run_id: String, item: state::QueueItemRow) -> bool {
    let key = queue_item_worker_key(&run_id, &item.id);
    let owner_id = queue_item_worker_owner_id(&key);
    let lease = match state::acquire_web_queue_item_worker_lease(
        &run_id,
        &item.id,
        &owner_id,
        WEB_QUEUE_WORKER_LEASE_TTL_SECS,
    ) {
        Ok(state::QueueWorkerLeaseAcquire::Acquired(lease)) => lease,
        Ok(
            state::QueueWorkerLeaseAcquire::Busy { .. }
            | state::QueueWorkerLeaseAcquire::Retryable { .. }
            | state::QueueWorkerLeaseAcquire::Terminal { .. }
            | state::QueueWorkerLeaseAcquire::Missing,
        ) => return false,
        Err(err) => {
            eprintln!("warning: failed to acquire queue worker lease for {key}: {err:#}");
            return false;
        }
    };
    let workers = WEB_QUEUE_ITEM_WORKERS.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut active) = workers.lock() {
        if !active.insert(key.clone()) {
            let _ = state::release_web_queue_item_worker_lease(&lease);
            return false;
        }
    }
    thread::spawn(move || {
        let item_id = item.id.clone();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_web_queue_item_with_lease(&run_id, &item, Some(&lease))
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
        let _ = state::release_web_queue_item_worker_lease(&lease);
        if let Some(workers) = WEB_QUEUE_ITEM_WORKERS.get() {
            if let Ok(mut active) = workers.lock() {
                active.remove(&key);
            }
        }
    });
    true
}

fn queue_item_worker_owner_id(key: &str) -> String {
    format!("pid:{}:{key}", std::process::id())
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
#[allow(dead_code, reason = "unit tests exercise the lease-free direct runner")]
fn run_web_queue_item(run_id: &str, item: &state::QueueItemRow) -> Result<QueueItemOutcome> {
    run_web_queue_item_with_lease(run_id, item, None)
}

fn queue_item_preflight_outcome_from_reduction(
    run_id: &str,
    item: &mut state::QueueItemRow,
    status: &str,
    evidence: &QueueStatusEvidence,
    reduction: &QueueStatusReduction,
) -> Result<Option<QueueItemOutcome>> {
    match reduction.allowed_action {
        QueueAllowedAction::None | QueueAllowedAction::RefreshEvidence => Ok(None),
        QueueAllowedAction::MarkSuccess => {
            update_successful_queue_item(run_id, item, item.agent_id.as_deref(), item.attempts)?;
            Ok(Some(QueueItemOutcome::Success))
        }
        QueueAllowedAction::MarkPaused => {
            if let Some(agent_id) = item.agent_id.as_deref() {
                return mark_queue_item_paused(run_id, item, agent_id, item.attempts, status)
                    .map(Some);
            }
            state::update_web_queue_item(
                run_id,
                &item.id,
                "paused",
                status,
                None,
                item.attempts,
                None,
            )?;
            state::update_web_queue_run(run_id, "stopped", item.position, status)?;
            Ok(Some(QueueItemOutcome::Stopped))
        }
        QueueAllowedAction::MarkRunning => {
            let agent_id = evidence
                .remote_live_agent_id
                .as_deref()
                .or(evidence.recovery_live_agent_id.as_deref())
                .or(item.agent_id.as_deref())
                .map(str::to_string);
            let message = queue_status_running_message(item, agent_id.as_deref(), evidence);
            state::update_web_queue_item(
                run_id,
                &item.id,
                "running",
                &message,
                agent_id.as_deref(),
                item.attempts,
                None,
            )?;
            state::update_web_queue_run(
                run_id,
                "running",
                item.position,
                queue_status_running_run_message(item, evidence, &message),
            )?;
            item.status = state::QueueItemStatus::Running;
            item.message = message;
            item.agent_id = agent_id;
            Ok(None)
        }
        QueueAllowedAction::RelaunchRemoteDisconnectedOpenRecord => {
            relaunch_remote_native_disconnected_item(run_id, item, item.attempts).map(Some)
        }
        QueueAllowedAction::RecoverExecution => {
            if evidence.recovery_live_agent_id.is_some()
                || evidence.recovery_waiting_on_current_attempt
            {
                return Ok(None);
            }
            let failure_message = queue_status_recovery_failure_message(status, evidence);
            fail_or_schedule_queue_item_recovery(
                run_id,
                item,
                failure_message,
                item.agent_id.as_deref(),
                item.attempts,
            )
            .map(Some)
        }
        QueueAllowedAction::MarkFailed => {
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                status,
                item.agent_id.as_deref(),
                item.attempts,
                None,
            )?;
            Ok(Some(QueueItemOutcome::failed(status)))
        }
        QueueAllowedAction::BoundedRelaunch => bounded_queue_item_relaunch(
            run_id,
            item,
            queue_launch_failed_before_record_message(item),
            item.attempts,
        )
        .map(Some),
    }
}

#[allow(clippy::too_many_lines, reason = "existing queue runner split debt")]
fn run_web_queue_item_with_lease(
    run_id: &str,
    item: &state::QueueItemRow,
    lease: Option<&state::QueueWorkerLease>,
) -> Result<QueueItemOutcome> {
    let mut item = item.clone();
    let run_view = status_reducer_run_view(run_id, &item);
    let evidence = collect_queue_status_evidence_for_item(&run_view, &item)?;
    let reduction = reduce_queue_status(&evidence);
    if let Some(status) = evidence.task_status.as_deref() {
        if let Some(outcome) = queue_item_preflight_outcome_from_reduction(
            run_id,
            &mut item,
            status,
            &evidence,
            &reduction,
        )? {
            return Ok(outcome);
        }
    } else if reduction.allowed_action == QueueAllowedAction::BoundedRelaunch {
        return bounded_queue_item_relaunch(
            run_id,
            &item,
            queue_launch_failed_before_record_message(&item),
            item.attempts,
        );
    } else if reduction.allowed_action == QueueAllowedAction::RefreshEvidence
        && item.status.is_starting_or_running()
    {
        update_queue_item_status_sync_unavailable_wait(
            run_id,
            &item,
            item.agent_id.as_deref(),
            item.attempts,
            reduction.reason,
        )?;
    }
    if item.status.is_starting_or_running() {
        if let Some(agent_id) = item.agent_id.as_deref() {
            if queue_item_remote_native(&item) {
                return wait_for_queue_item_closeout(run_id, &item, agent_id, item.attempts, lease);
            }
            if agent_running(agent_id) {
                if agent_terminal_closeout_failed(agent_id) {
                    return fail_or_schedule_queue_item_recovery(
                        run_id,
                        &item,
                        QUEUE_AGENT_FAILED_QCOLD_CLOSEOUT,
                        Some(agent_id),
                        item.attempts,
                    );
                }
                return wait_for_queue_item_closeout(run_id, &item, agent_id, item.attempts, lease);
            }
            return fail_or_schedule_queue_item_recovery(
                run_id,
                &item,
                QUEUE_AGENT_EXITED_BEFORE_CLOSEOUT,
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
                let resumed_item = remote_native_running_wait_item(&item, agent_id);
                return wait_for_queue_item_closeout(
                    run_id,
                    &resumed_item,
                    agent_id,
                    item.attempts,
                    lease,
                );
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
                return wait_for_queue_item_closeout(run_id, &item, agent_id, item.attempts, lease);
            }
        }
    }
    let mut retries = item.attempts.max(0);
    loop {
        if state::web_queue_stop_requested(run_id)? {
            pause_web_queue_item(run_id, &item, None, retries)?;
            return Ok(QueueItemOutcome::Stopped);
        }
        heartbeat_queue_item_worker_lease(lease)?;

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
            let outcome = start_remote_native_queue_item(run_id, &mut item, retries, lease)?;
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
                        if !sleep_queue_retry(run_id, &item.id, delay)? {
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
                        if !sleep_queue_retry(run_id, &item.id, delay)? {
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
        let outcome = start_web_queue_item(run_id, &item, retries, lease)?;
        if let Some(outcome) = handle_queue_launch_outcome(run_id, &mut item, &mut retries, outcome)? {
            return Ok(outcome);
        }
    }
}

fn start_web_queue_item(
    run_id: &str,
    item: &state::QueueItemRow,
    attempts: i64,
    lease: Option<&state::QueueWorkerLease>,
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
    let outcome = wait_for_queue_item_closeout(run_id, item, &agent.id, attempts, lease);
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
        if run.status.is_active() {
            state::wake_web_queue_retry_items(run_id)?;
            spawn_web_queue_worker(run_id.to_string());
        }
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
    } else if run.status == "stopped" {
        state::continue_web_queue_run(run_id)?;
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
