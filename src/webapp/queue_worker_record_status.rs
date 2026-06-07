const REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE: &str =
    "remote-native task is open, but remote-agent session is not running on the remote host";
const REMOTE_NATIVE_OPEN_RECORD_RELAUNCH_MESSAGE: &str =
    "remote-native task is open but remote-agent session is missing; relaunching item";
const LOCAL_OPEN_RECORD_RECOVERY_MESSAGE: &str =
    "local task is open but agent session is missing";
const LOCAL_OPEN_RECORD_STOPPED_MESSAGE: &str =
    "local task is open but agent session is missing; press Continue to resume";
const QUEUE_CONTINUE_PENDING_MESSAGE: &str = "pending after queue continue";

fn execute_queue_status_reduction(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    status: &str,
    evidence: &QueueStatusEvidence,
    reduction: &QueueStatusReduction,
    changed: &mut bool,
    terminal_run: &mut Option<(String, i64, String)>,
) -> Result<bool> {
    match reduction.allowed_action {
        QueueAllowedAction::None | QueueAllowedAction::RefreshEvidence => Ok(reduction.handled),
        QueueAllowedAction::MarkSuccess => {
            mark_queue_status_success(run, item, changed)?;
            Ok(true)
        }
        QueueAllowedAction::MarkPaused => {
            mark_queue_status_paused(run, item, status, changed, terminal_run)?;
            Ok(true)
        }
        QueueAllowedAction::MarkRunning => {
            mark_queue_status_running(run, item, evidence, changed)?;
            Ok(true)
        }
        QueueAllowedAction::RelaunchRemoteDisconnectedOpenRecord => {
            relaunch_remote_native_disconnected_item(&run.id, item, item.attempts)?;
            *changed = true;
            Ok(true)
        }
        QueueAllowedAction::RecoverExecution => {
            recover_queue_status_execution(run, item, status, evidence, changed, terminal_run)?;
            Ok(true)
        }
        QueueAllowedAction::MarkFailed => {
            mark_queue_status_failed(run, item, status, changed, terminal_run)?;
            Ok(true)
        }
        QueueAllowedAction::BoundedRelaunch => Ok(false),
    }
}

fn mark_queue_status_success(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    changed: &mut bool,
) -> Result<()> {
    if !item.status.is_success() || item.agent_id.as_deref().is_some_and(agent_running) {
        update_successful_queue_item(&run.id, item, item.agent_id.as_deref(), item.attempts)?;
        *changed = true;
    }
    Ok(())
}

fn mark_queue_status_paused(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    status: &str,
    changed: &mut bool,
    terminal_run: &mut Option<(String, i64, String)>,
) -> Result<()> {
    if !item.status.is_paused() {
        state::update_web_queue_item(
            &run.id,
            &item.id,
            "paused",
            status,
            item.agent_id.as_deref(),
            item.attempts,
            None,
        )?;
        *changed = true;
    }
    terminal_run.get_or_insert(("stopped".into(), item.position, status.to_string()));
    Ok(())
}

fn mark_queue_status_running(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    evidence: &QueueStatusEvidence,
    changed: &mut bool,
) -> Result<()> {
    let agent_id = evidence
        .remote_live_agent_id
        .as_deref()
        .or(evidence.recovery_live_agent_id.as_deref())
        .or(item.agent_id.as_deref());
    let message = queue_status_running_message(item, agent_id, evidence);
    if !item.status.is_running() || item.message != message || item.agent_id.as_deref() != agent_id
    {
        state::update_web_queue_item(
            &run.id,
            &item.id,
            "running",
            &message,
            agent_id,
            item.attempts,
            None,
        )?;
        *changed = true;
    }
    state::update_web_queue_run(
        &run.id,
        "running",
        item.position,
        queue_status_running_run_message(item, evidence, &message),
    )
}

fn recover_queue_status_execution(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    status: &str,
    evidence: &QueueStatusEvidence,
    changed: &mut bool,
    terminal_run: &mut Option<(String, i64, String)>,
) -> Result<()> {
    if let Some(agent_id) = evidence.recovery_live_agent_id.as_deref() {
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
        return Ok(());
    }
    if evidence.recovery_waiting_on_current_attempt {
        return Ok(());
    }
    let failure_message = queue_status_recovery_failure_message(status, evidence);
    if schedule_queue_item_auto_recovery(&run.id, item, failure_message)? {
        *changed = true;
        return Ok(());
    }
    let message = exhausted_queue_item_failure_message(item, failure_message)?;
    state::update_web_queue_item(
        &run.id,
        &item.id,
        "failed",
        &message,
        item.agent_id.as_deref(),
        item.attempts,
        None,
    )?;
    *changed = true;
    terminal_run.get_or_insert(("failed".into(), item.position, message));
    Ok(())
}

fn mark_queue_status_failed(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    status: &str,
    changed: &mut bool,
    terminal_run: &mut Option<(String, i64, String)>,
) -> Result<()> {
    if item.status.is_success() {
        return Ok(());
    }
    let message = if queue_status_auto_recoverable(status) {
        exhausted_queue_item_failure_message(item, status)?
    } else {
        status.to_string()
    };
    state::update_web_queue_item(
        &run.id,
        &item.id,
        "failed",
        &message,
        item.agent_id.as_deref(),
        item.attempts,
        None,
    )?;
    *changed = true;
    terminal_run.get_or_insert(("failed".into(), item.position, message));
    Ok(())
}

fn queue_status_running_message(
    item: &state::QueueItemRow,
    agent_id: Option<&str>,
    evidence: &QueueStatusEvidence,
) -> String {
    if evidence.recovery_live_agent_id.as_deref() == agent_id {
        return agent_id.map_or_else(
            || "running recovery retry".to_string(),
            |agent_id| format!("running recovery retry ({agent_id})"),
        );
    }
    if evidence.remote_native {
        return agent_id.map_or_else(
            || "remote-native session is running".to_string(),
            |agent_id| {
                if item.status.is_stopped_or_paused() {
                    format!("resumed remote-native agent {agent_id}")
                } else {
                    remote_native_active_open_message(item, agent_id)
                }
            },
        );
    }
    agent_id.map_or_else(
        || "local agent is running".to_string(),
        |agent_id| format!("{} ({agent_id})", queue_display_label(item)),
    )
}

fn queue_status_running_run_message<'a>(
    item: &state::QueueItemRow,
    evidence: &QueueStatusEvidence,
    item_message: &'a str,
) -> &'a str {
    if evidence.remote_native && item.status.is_stopped_or_paused() {
        "running"
    } else {
        item_message
    }
}

fn local_open_record_launch_in_progress(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
) -> bool {
    item.status.is_starting_or_running()
        && item.agent_id.is_none()
        && queue_item_worker_active(&run.id, &item.id)
}

fn remote_native_active_open_message(item: &state::QueueItemRow, agent_id: &str) -> String {
    format!("{} ({agent_id})", queue_display_label(item))
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

#[cfg(test)]
fn queue_item_status_closeout_outcome(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
    status: String,
) -> Result<Option<QueueItemOutcome>> {
    let run = status_reducer_run_view(run_id, item);
    let evidence = collect_queue_status_evidence(&run, item, Some(status.clone()), None);
    let reduction = reduce_queue_status(&evidence);
    queue_item_closeout_outcome_from_reduction(
        run_id,
        item,
        agent_id,
        attempts,
        &status,
        &evidence,
        &reduction,
    )
}

fn status_reducer_run_view(run_id: &str, item: &state::QueueItemRow) -> state::QueueRunRow {
    state::QueueRunRow {
        id: run_id.to_string(),
        status: state::QueueRunStatus::Running,
        execution_mode: "sequence".into(),
        execution_host: item.execution_host.clone(),
        selected_agent_command: item.agent_command.clone(),
        remote_launcher: item.remote_launcher.clone(),
        remote_agent_local_proxy: item.remote_agent_local_proxy.clone(),
        remote_agent_remote_proxy: item.remote_agent_remote_proxy.clone(),
        selected_repo_root: item.repo_root.clone(),
        selected_repo_name: item.repo_name.clone(),
        track: "queue-run".to_string(),
        current_index: item.position,
        stop_requested: false,
        message: String::new(),
        created_at: 0,
        updated_at: 0,
    }
}

fn queue_item_closeout_outcome_from_reduction(
    run_id: &str,
    item: &state::QueueItemRow,
    agent_id: &str,
    attempts: i64,
    status: &str,
    evidence: &QueueStatusEvidence,
    reduction: &QueueStatusReduction,
) -> Result<Option<QueueItemOutcome>> {
    match reduction.allowed_action {
        QueueAllowedAction::None | QueueAllowedAction::RefreshEvidence => {
            if status == "open"
                && !queue_item_remote_native(item)
                && agent_running(agent_id)
            {
                let _ = submit_agent_terminal_pending_paste(agent_id);
            }
            Ok(None)
        }
        QueueAllowedAction::MarkSuccess => {
            update_successful_queue_item(run_id, item, Some(agent_id), attempts)?;
            Ok(Some(QueueItemOutcome::Success))
        }
        QueueAllowedAction::MarkPaused => {
            mark_queue_item_paused(run_id, item, agent_id, attempts, status).map(Some)
        }
        QueueAllowedAction::MarkRunning => {
            let live_agent_id = evidence
                .remote_live_agent_id
                .as_deref()
                .or(evidence.recovery_live_agent_id.as_deref())
                .unwrap_or(agent_id);
            let message = queue_status_running_message(item, Some(live_agent_id), evidence);
            state::update_web_queue_item(
                run_id,
                &item.id,
                "running",
                &message,
                Some(live_agent_id),
                attempts,
                None,
            )?;
            state::update_web_queue_run(
                run_id,
                "running",
                item.position,
                queue_status_running_run_message(item, evidence, &message),
            )?;
            Ok(None)
        }
        QueueAllowedAction::RelaunchRemoteDisconnectedOpenRecord => {
            relaunch_remote_native_disconnected_item(run_id, item, attempts).map(Some)
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
                Some(agent_id),
                attempts,
            )
            .map(Some)
        }
        QueueAllowedAction::MarkFailed => {
            fail_queue_item_from_task_status(
                run_id,
                item,
                agent_id,
                attempts,
                status.to_string(),
            )
            .map(Some)
        }
        QueueAllowedAction::BoundedRelaunch => Ok(None),
    }
}

fn queue_status_recovery_failure_message<'a>(
    status: &'a str,
    evidence: &QueueStatusEvidence,
) -> &'a str {
    if evidence.closeout_failed_prompt_live {
        QUEUE_AGENT_FAILED_QCOLD_CLOSEOUT
    } else if status == "open" && !evidence.remote_native {
        LOCAL_OPEN_RECORD_RECOVERY_MESSAGE
    } else {
        status
    }
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
