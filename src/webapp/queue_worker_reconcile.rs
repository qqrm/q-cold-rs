fn start_web_queue_status_reconciler() {
    WEB_QUEUE_STATUS_RECONCILER.get_or_init(|| {
        thread::spawn(|| loop {
            reconcile_stale_web_queue_run_soon();
            thread::sleep(web_queue_status_sync_interval());
        });
    });
}

fn web_queue_status_sync_interval() -> Duration {
    let seconds = env::var(WEB_QUEUE_STATUS_SYNC_INTERVAL_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(WEB_QUEUE_STATUS_SYNC_INTERVAL_SECS);
    Duration::from_secs(seconds)
}

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
    if run.status.is_active()
        || items.iter().any(|item| {
            item.status.is_starting_or_running()
            || (item.status.is_success()
                && item
                    .agent_id
                    .as_deref()
                    .is_some_and(agent_running))
        })
        || stopped_local_open_record_may_need_reconcile(run, items)?
    {
        return Ok(true);
    }
    failed_queue_run_may_be_resolved(run, items)
}

fn failed_queue_run_may_be_resolved(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Result<bool> {
    if !run.status.is_failed() || items.is_empty() {
        return Ok(false);
    }
    if !items
        .iter()
        .any(|item| item.status.is_failed_or_blocked())
    {
        return Ok(true);
    }
    for item in items {
        if !item.status.is_failed_or_blocked() {
            continue;
        }
        if queue_item_recovery_waiting_on_current_attempt(item) {
            return Ok(true);
        }
        if queue_failure_message_auto_recoverable(&item.message)
            && item.recovery_attempts < WEB_QUEUE_AUTO_RECOVERY_ATTEMPTS
        {
            return Ok(true);
        }
        if remote_native_retry_session_running(item) {
            return Ok(true);
        }
        if queue_failure_message_launch_recoverable(&item.message) {
            return Ok(true);
        }
        let evidence = collect_queue_status_evidence_for_item(run, item)?;
        let reduction = reduce_queue_status(&evidence);
        match reduction.allowed_action {
            QueueAllowedAction::MarkSuccess
            | QueueAllowedAction::MarkRunning
            | QueueAllowedAction::RelaunchRemoteDisconnectedOpenRecord
            | QueueAllowedAction::BoundedRelaunch => return Ok(true),
            QueueAllowedAction::RecoverExecution
                if item.recovery_attempts < WEB_QUEUE_AUTO_RECOVERY_ATTEMPTS =>
            {
                return Ok(true);
            }
            _ => {}
        }
    }
    Ok(false)
}

fn stopped_local_open_record_may_need_reconcile(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Result<bool> {
    if run.status != state::QueueRunStatus::Stopped {
        return Ok(false);
    }
    for item in items {
        if item.status != state::QueueItemStatus::Stopped
            || item.message != LOCAL_OPEN_RECORD_STOPPED_MESSAGE
        {
            continue;
        }
        let evidence = collect_queue_status_evidence_for_item(run, item)?;
        let reduction = reduce_queue_status(&evidence);
        if matches!(
            reduction.allowed_action,
            QueueAllowedAction::RecoverExecution | QueueAllowedAction::MarkRunning
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn queue_agent_failure_message(item: &state::QueueItemRow, agent_id: &str) -> Option<&'static str> {
    if !item.status.is_starting_or_running() {
        return None;
    }
    if queue_item_remote_native(item) {
        return None;
    }
    if !agent_running(agent_id) {
        return Some(QUEUE_AGENT_EXITED_BEFORE_CLOSEOUT);
    }
    if agent_terminal_closeout_failed(agent_id) {
        return Some(QUEUE_AGENT_FAILED_QCOLD_CLOSEOUT);
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
    if !item.status.is_failed_or_blocked() {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaleTaskRecordHandling {
    Unhandled,
    Handled,
    Active,
    Terminal,
}

fn resume_stale_active_queue_run(
    run: &state::QueueRunRow,
    items: Vec<state::QueueItemRow>,
) -> Result<()> {
    for item in items {
        match stale_queue_task_record_handled(run, &item)? {
            StaleTaskRecordHandling::Unhandled => {}
            StaleTaskRecordHandling::Handled => continue,
            StaleTaskRecordHandling::Active => {
                spawn_web_queue_worker(run.id.clone());
                return Ok(());
            }
            StaleTaskRecordHandling::Terminal => return Ok(()),
        }
        if item.status.is_success() {
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
        if item.status.is_failed_or_blocked() {
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
    if !run.status.is_failed() {
        return Ok(None);
    }
    if items
        .iter()
        .any(|item| item.status.is_failed_or_blocked())
    {
        return Ok(None);
    }
    if items.iter().all(|item| item.status.is_success()) {
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
) -> Result<StaleTaskRecordHandling> {
    let evidence = collect_queue_status_evidence_for_item(run, item)?;
    let reduction = reduce_queue_status(&evidence);
    let Some(status) = evidence.task_status.as_deref() else {
        if reduction.allowed_action == QueueAllowedAction::BoundedRelaunch {
            let outcome = bounded_queue_item_relaunch(
                &run.id,
                item,
                queue_launch_failed_before_record_message(item),
                item.attempts,
            )?;
            return Ok(match outcome {
                QueueItemOutcome::RecoveryScheduled => StaleTaskRecordHandling::Active,
                QueueItemOutcome::Failed { .. } | QueueItemOutcome::Stopped => {
                    StaleTaskRecordHandling::Terminal
                }
                QueueItemOutcome::Success => StaleTaskRecordHandling::Handled,
            });
        }
        if reduction.allowed_action == QueueAllowedAction::RefreshEvidence {
            if item.status.is_starting_or_running() {
                update_queue_item_status_sync_unavailable_wait(
                    &run.id,
                    item,
                    item.agent_id.as_deref(),
                    item.attempts,
                    reduction.reason,
                )?;
            }
            return Ok(StaleTaskRecordHandling::Active);
        }
        if reconcile_remote_native_retry(run, item)? {
            state::update_web_queue_run(
                &run.id,
                "running",
                item.position,
                REMOTE_NATIVE_RETRY_RUNNING_MESSAGE,
            )?;
            return Ok(StaleTaskRecordHandling::Active);
        }
        return Ok(stale_reduction_handling(&reduction));
    };
    let mut changed = false;
    let mut terminal_run = None;
    let handled = execute_queue_status_reduction(
        run,
        item,
        status,
        &evidence,
        &reduction,
        &mut changed,
        &mut terminal_run,
    )?;
    let terminal = terminal_run.is_some();
    if let Some((status, position, message)) = terminal_run {
        state::update_web_queue_run(&run.id, &status, position, &message)?;
    }
    if terminal {
        return Ok(StaleTaskRecordHandling::Terminal);
    }
    if !handled {
        return Ok(StaleTaskRecordHandling::Unhandled);
    }
    Ok(stale_reduction_handling(&reduction))
}

fn stale_reduction_handling(reduction: &QueueStatusReduction) -> StaleTaskRecordHandling {
    if !reduction.handled {
        return StaleTaskRecordHandling::Unhandled;
    }
    match reduction.effective_status {
        QueueEffectiveStatus::Running
        | QueueEffectiveStatus::WaitingForRecord
        | QueueEffectiveStatus::StaleUnknown
        | QueueEffectiveStatus::ExecutionFailed
        | QueueEffectiveStatus::CloseoutFailedButSessionLive
        | QueueEffectiveStatus::DisconnectedOpenRecord
        | QueueEffectiveStatus::LaunchFailed => StaleTaskRecordHandling::Active,
        QueueEffectiveStatus::ClosedSuccess => StaleTaskRecordHandling::Handled,
        QueueEffectiveStatus::Paused | QueueEffectiveStatus::TerminalFailure => {
            StaleTaskRecordHandling::Terminal
        }
    }
}

fn close_running_success_agent(run: &state::QueueRunRow, item: &state::QueueItemRow) -> Result<()> {
    if item.agent_id.as_deref().is_some_and(agent_running) {
        update_successful_queue_item(&run.id, item, item.agent_id.as_deref(), item.attempts)?;
    }
    Ok(())
}
