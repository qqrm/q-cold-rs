#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueueEffectiveStatus {
    Running,
    WaitingForRecord,
    StaleUnknown,
    DisconnectedOpenRecord,
    ClosedSuccess,
    ExecutionFailed,
    CloseoutFailedButSessionLive,
    LaunchFailed,
    Paused,
    TerminalFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueueAllowedAction {
    None,
    RefreshEvidence,
    MarkSuccess,
    MarkPaused,
    MarkRunning,
    RelaunchRemoteDisconnectedOpenRecord,
    RecoverExecution,
    BoundedRelaunch,
    MarkFailed,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "status evidence intentionally stores independent observed predicates"
)]
#[derive(Clone, Debug)]
struct QueueStatusEvidence {
    task_status: Option<String>,
    status_sync_error: Option<String>,
    item_status: state::QueueItemStatus,
    item_message: String,
    remote_native: bool,
    has_agent_id: bool,
    local_agent_live: bool,
    remote_live_agent_id: Option<String>,
    closeout_failed_prompt_live: bool,
    launch_in_progress: bool,
    recovery_waiting_on_current_attempt: bool,
    recovery_live_agent_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QueueStatusReduction {
    effective_status: QueueEffectiveStatus,
    reason: &'static str,
    allowed_action: QueueAllowedAction,
    handled: bool,
}

impl QueueStatusReduction {
    fn new(
        effective_status: QueueEffectiveStatus,
        reason: &'static str,
        allowed_action: QueueAllowedAction,
        handled: bool,
    ) -> Self {
        Self {
            effective_status,
            reason,
            allowed_action,
            handled,
        }
    }
}

fn collect_queue_status_evidence(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
    task_status: Option<String>,
    status_sync_error: Option<String>,
) -> QueueStatusEvidence {
    let remote_native = queue_item_remote_native(item);
    let agent_id = item.agent_id.as_deref().filter(|value| !value.trim().is_empty());
    let needs_remote_liveness = task_status.as_deref().is_none_or(|status| {
        status == "open" || queue_status_auto_recoverable(status)
    });
    let remote_live_agent_id = (remote_native && needs_remote_liveness)
        .then(|| remote_native_open_record_live_agent_id(item))
        .flatten();
    let local_agent_live = !remote_native && agent_id.is_some_and(agent_running);
    let closeout_failed_prompt_live =
        local_agent_live && agent_id.is_some_and(agent_terminal_closeout_failed);
    QueueStatusEvidence {
        task_status,
        status_sync_error,
        item_status: item.status.clone(),
        item_message: item.message.clone(),
        remote_native,
        has_agent_id: agent_id.is_some(),
        local_agent_live,
        remote_live_agent_id,
        closeout_failed_prompt_live,
        launch_in_progress: local_open_record_launch_in_progress(run, item),
        recovery_waiting_on_current_attempt: queue_item_recovery_waiting_on_current_attempt(item),
        recovery_live_agent_id: live_queue_item_recovery_agent_id(item).map(str::to_string),
    }
}

fn collect_queue_status_evidence_for_item(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
) -> Result<QueueStatusEvidence> {
    match queue_task_status(item) {
        Ok(status) => Ok(collect_queue_status_evidence(run, item, status, None)),
        Err(err) if queue_task_status_error_is_stale_unknown(&err) => Ok(
            collect_queue_status_evidence(run, item, None, Some(format!("{err:#}"))),
        ),
        Err(err) => Err(err),
    }
}

fn queue_task_status_error_is_stale_unknown(err: &anyhow::Error) -> bool {
    queue_task_status_message_is_stale_unknown(&format!("{err:#}"))
}

fn queue_task_status_message_is_stale_unknown(message: &str) -> bool {
    message.contains("remote-native task-record sync failed")
        || message.contains("task-record sync failed")
        || message.contains("failed to sync remote task records")
}

fn reduce_queue_status(evidence: &QueueStatusEvidence) -> QueueStatusReduction {
    let Some(status) = evidence.task_status.as_deref() else {
        if evidence.has_agent_id
            && !evidence.local_agent_live
            && evidence.remote_live_agent_id.is_none()
            && missing_record_can_relaunch(evidence)
        {
            return QueueStatusReduction::new(
                QueueEffectiveStatus::LaunchFailed,
                "executor vanished before a task record was visible",
                QueueAllowedAction::BoundedRelaunch,
                false,
            );
        }
        if evidence.status_sync_error.is_some() {
            return QueueStatusReduction::new(
                QueueEffectiveStatus::StaleUnknown,
                "task-record sync unavailable; keep queue row stale instead of failing it",
                QueueAllowedAction::RefreshEvidence,
                true,
            );
        }
        if evidence.remote_native && evidence.remote_live_agent_id.is_some() {
            return QueueStatusReduction::new(
                QueueEffectiveStatus::WaitingForRecord,
                "remote-native session is live but task record is not visible yet",
                QueueAllowedAction::None,
                true,
            );
        }
        return QueueStatusReduction::new(
            QueueEffectiveStatus::WaitingForRecord,
            "task record is not visible yet",
            QueueAllowedAction::None,
            false,
        );
    };

    if status == "closed:success" {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::ClosedSuccess,
            "task record closed successfully",
            QueueAllowedAction::MarkSuccess,
            true,
        );
    }
    if status == "paused" {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::Paused,
            "task record is paused",
            QueueAllowedAction::MarkPaused,
            true,
        );
    }
    if status == "open" {
        return reduce_open_task_record(evidence);
    }
    if queue_status_auto_recoverable(status) {
        return reduce_recoverable_task_failure(evidence);
    }
    if queue_task_status_terminal(status) {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::TerminalFailure,
            "task record reached a terminal non-success status",
            QueueAllowedAction::MarkFailed,
            true,
        );
    }
    QueueStatusReduction::new(
        QueueEffectiveStatus::Running,
        "task record is active",
        QueueAllowedAction::None,
        true,
    )
}

fn reduce_open_task_record(evidence: &QueueStatusEvidence) -> QueueStatusReduction {
    if evidence.remote_native {
        if evidence.remote_live_agent_id.is_some() {
            return QueueStatusReduction::new(
                QueueEffectiveStatus::Running,
                "remote-native task record is open and a matching session is live",
                QueueAllowedAction::MarkRunning,
                true,
            );
        }
        if remote_native_open_record_should_relaunch(evidence) {
            return QueueStatusReduction::new(
                QueueEffectiveStatus::DisconnectedOpenRecord,
                "remote-native task record is open but no matching session is live",
                QueueAllowedAction::RelaunchRemoteDisconnectedOpenRecord,
                true,
            );
        }
        return QueueStatusReduction::new(
            QueueEffectiveStatus::DisconnectedOpenRecord,
            "remote-native task record is open but no matching session is live",
            QueueAllowedAction::None,
            false,
        );
    }
    if evidence.closeout_failed_prompt_live {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::CloseoutFailedButSessionLive,
            "local agent is live at a failed closeout prompt",
            QueueAllowedAction::RecoverExecution,
            true,
        );
    }
    if evidence.local_agent_live {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::Running,
            "local task record is open and the local agent is live",
            QueueAllowedAction::None,
            true,
        );
    }
    if evidence.launch_in_progress || evidence.recovery_waiting_on_current_attempt {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::Running,
            "open local task record is already covered by a launch or recovery worker",
            QueueAllowedAction::None,
            true,
        );
    }
    if evidence.item_status == state::QueueItemStatus::Pending
        && evidence.item_message == QUEUE_CONTINUE_PENDING_MESSAGE
    {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::Running,
            "queue continue already reset the disconnected local record for relaunch",
            QueueAllowedAction::None,
            true,
        );
    }
    if local_open_record_without_live_agent_recoverable(evidence) {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::ExecutionFailed,
            "local task record is open but no local agent is live",
            QueueAllowedAction::RecoverExecution,
            true,
        );
    }
    QueueStatusReduction::new(
        QueueEffectiveStatus::DisconnectedOpenRecord,
        "local task record is open but no local agent is live",
        QueueAllowedAction::None,
        false,
    )
}

fn remote_native_open_record_should_relaunch(evidence: &QueueStatusEvidence) -> bool {
    matches!(
        evidence.item_status,
        state::QueueItemStatus::Starting
            | state::QueueItemStatus::Running
            | state::QueueItemStatus::Waiting
    ) || (evidence.item_status.is_stopped_or_paused()
        && evidence.item_message == REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE)
        || (evidence.item_status.is_failed_or_blocked()
            && queue_task_status_message_is_stale_unknown(&evidence.item_message))
}

fn missing_record_can_relaunch(evidence: &QueueStatusEvidence) -> bool {
    matches!(
        evidence.item_status,
        state::QueueItemStatus::Starting
            | state::QueueItemStatus::Running
            | state::QueueItemStatus::Waiting
    )
}

fn local_open_record_without_live_agent_recoverable(evidence: &QueueStatusEvidence) -> bool {
    let waiting_for_continue = evidence.item_status == state::QueueItemStatus::Pending
        && evidence.item_message == QUEUE_CONTINUE_PENDING_MESSAGE;
    if evidence.item_status.is_success()
        || evidence.item_status.is_paused()
        || waiting_for_continue
        || evidence.launch_in_progress
    {
        return false;
    }
    !evidence.item_status.is_stopped_or_paused()
        || (evidence.item_status == state::QueueItemStatus::Stopped
            && evidence.item_message == LOCAL_OPEN_RECORD_STOPPED_MESSAGE)
}

fn reduce_recoverable_task_failure(evidence: &QueueStatusEvidence) -> QueueStatusReduction {
    if evidence.remote_native && evidence.remote_live_agent_id.is_some() {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::CloseoutFailedButSessionLive,
            "remote-native task record reports failed closeout while a matching session is live",
            QueueAllowedAction::MarkRunning,
            true,
        );
    }
    if evidence.recovery_live_agent_id.is_some() {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::Running,
            "semantic recovery agent is already live",
            QueueAllowedAction::MarkRunning,
            true,
        );
    }
    if evidence.recovery_waiting_on_current_attempt {
        return QueueStatusReduction::new(
            QueueEffectiveStatus::Running,
            "semantic recovery is already pending for the current attempt",
            QueueAllowedAction::None,
            true,
        );
    }
    QueueStatusReduction::new(
        QueueEffectiveStatus::ExecutionFailed,
        "task execution failed and may consume semantic recovery budget",
        QueueAllowedAction::RecoverExecution,
        true,
    )
}

#[cfg(test)]
mod queue_status_reducer_tests {
    use super::*;

    fn evidence(task_status: Option<&str>) -> QueueStatusEvidence {
        QueueStatusEvidence {
            task_status: task_status.map(str::to_string),
            status_sync_error: None,
            item_status: state::QueueItemStatus::Running,
            item_message: String::new(),
            remote_native: false,
            has_agent_id: true,
            local_agent_live: true,
            remote_live_agent_id: None,
            closeout_failed_prompt_live: false,
            launch_in_progress: false,
            recovery_waiting_on_current_attempt: false,
            recovery_live_agent_id: None,
        }
    }

    #[test]
    fn reducer_keeps_sync_unavailable_as_stale_unknown() {
        let mut evidence = evidence(None);
        evidence.has_agent_id = false;
        evidence.local_agent_live = false;
        evidence.status_sync_error = Some("sync failed".to_string());

        let reduction = reduce_queue_status(&evidence);

        assert_eq!(reduction.effective_status, QueueEffectiveStatus::StaleUnknown);
        assert_eq!(reduction.allowed_action, QueueAllowedAction::RefreshEvidence);
    }

    #[test]
    fn reducer_bounds_relaunch_only_for_missing_record_after_executor_vanishes() {
        let mut evidence = evidence(None);
        evidence.local_agent_live = false;
        evidence.remote_live_agent_id = None;
        evidence.item_status = state::QueueItemStatus::Running;

        let reduction = reduce_queue_status(&evidence);

        assert_eq!(reduction.effective_status, QueueEffectiveStatus::LaunchFailed);
        assert_eq!(reduction.allowed_action, QueueAllowedAction::BoundedRelaunch);
    }

    #[test]
    fn reducer_relaunches_remote_open_without_session() {
        let mut evidence = evidence(Some("open"));
        evidence.remote_native = true;
        evidence.local_agent_live = false;
        evidence.remote_live_agent_id = None;

        let reduction = reduce_queue_status(&evidence);

        assert_eq!(
            reduction.effective_status,
            QueueEffectiveStatus::DisconnectedOpenRecord
        );
        assert_eq!(
            reduction.allowed_action,
            QueueAllowedAction::RelaunchRemoteDisconnectedOpenRecord
        );
    }

    #[test]
    fn reducer_limits_recovery_to_execution_failure() {
        let mut evidence = evidence(Some("closed:failed"));
        evidence.local_agent_live = false;

        let reduction = reduce_queue_status(&evidence);

        assert_eq!(reduction.effective_status, QueueEffectiveStatus::ExecutionFailed);
        assert_eq!(reduction.allowed_action, QueueAllowedAction::RecoverExecution);
    }

    #[test]
    fn reducer_keeps_failed_closeout_live_session_running() {
        let mut evidence = evidence(Some("failed-closeout"));
        evidence.remote_native = true;
        evidence.local_agent_live = false;
        evidence.remote_live_agent_id = Some("agent-repair".to_string());

        let reduction = reduce_queue_status(&evidence);

        assert_eq!(
            reduction.effective_status,
            QueueEffectiveStatus::CloseoutFailedButSessionLive
        );
        assert_eq!(reduction.allowed_action, QueueAllowedAction::MarkRunning);
    }
}
