fn queue_task_status(item: &state::QueueItemRow) -> Result<Option<String>> {
    let task_id = format!("task/{}", item.slug);
    let local_status = queue_task_status_from_local_records(item, &task_id)?;
    if item.remote_launcher.is_some() {
        let required_remote_native_sync = remote_native_requires_task_record_sync(item);
        if !required_remote_native_sync && local_status.supersedes_optional_remote_sync() {
            return Ok(local_status.status);
        }
        let sync_result = sync_remote_queue_task_records(item, required_remote_native_sync);
        if let Err(err) = sync_result {
            if required_remote_native_sync {
                match remote_native_sync_failure_fallback_status(item, &task_id)? {
                    Some(RemoteNativeSyncFallback::Status(status)) => return Ok(Some(status)),
                    Some(RemoteNativeSyncFallback::PendingRecovery) => return Ok(None),
                    None => {}
                }
                return Err(err).context("remote-native task-record sync failed");
            }
        }
    }
    Ok(queue_task_status_from_local_records(item, &task_id)?.status)
}

struct QueueTaskLocalStatus {
    status: Option<String>,
    from_recovery: bool,
}

impl QueueTaskLocalStatus {
    fn none() -> Self {
        Self {
            status: None,
            from_recovery: false,
        }
    }

    fn from_status(status: Option<String>) -> Self {
        Self {
            status,
            from_recovery: false,
        }
    }

    fn from_recovery_status(status: String) -> Self {
        Self {
            status: Some(status),
            from_recovery: true,
        }
    }

    fn supersedes_optional_remote_sync(&self) -> bool {
        self.from_recovery
            && self.status.as_deref().is_some_and(|status| {
                status == "closed:success" || !queue_task_status_terminal(status)
            })
    }
}

fn queue_task_status_from_local_records(
    item: &state::QueueItemRow,
    task_id: &str,
) -> Result<QueueTaskLocalStatus> {
    let record = match state::get_task_record(task_id)? {
        Some(record) if queue_task_record_matches_item(item, &record) => Some(record),
        Some(_) => return Ok(QueueTaskLocalStatus::none()),
        None => None,
    };
    queue_task_status_from_record_or_recovery(item, task_id, record.as_ref())
}

fn queue_task_status_from_record_or_recovery(
    item: &state::QueueItemRow,
    task_id: &str,
    record: Option<&state::TaskRecordRow>,
) -> Result<QueueTaskLocalStatus> {
    let recovery = latest_related_recovery_task_record(item, task_id)?;
    if let Some(recovery) = recovery.as_ref() {
        if record.is_none_or(|record| recovery.updated_at >= record.updated_at) {
            if remote_native_failed_closeout_is_being_retried(item, &recovery.status) {
                return Ok(QueueTaskLocalStatus::none());
            }
            return Ok(QueueTaskLocalStatus::from_recovery_status(
                recovery.status.clone(),
            ));
        }
    }
    if record.is_some_and(|record| {
        remote_native_failed_closeout_is_being_retried(item, &record.status)
    }) {
        return Ok(QueueTaskLocalStatus::none());
    }
    Ok(QueueTaskLocalStatus::from_status(
        record.map(|record| record.status.clone()),
    ))
}

fn latest_related_recovery_task_record(
    item: &state::QueueItemRow,
    task_id: &str,
) -> Result<Option<state::TaskRecordRow>> {
    let Some(repo_root) = item.repo_root.as_deref().filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let records = state::load_task_records_for_repo(repo_root, None, 128)?;
    Ok(records
        .into_iter()
        .filter(|record| {
            related_recovery_task_record_id(task_id, &record.id)
                && related_recovery_task_record_matches_item(item, record)
                && (item.started_at == 0 || record.updated_at >= item.started_at)
        })
        .max_by_key(recovery_task_record_precedence))
}

fn related_recovery_task_record_matches_item(
    item: &state::QueueItemRow,
    record: &state::TaskRecordRow,
) -> bool {
    if queue_task_record_matches_item(item, record) {
        return true;
    }
    related_manual_repair_task_record_matches_item(item, record)
}

fn related_manual_repair_task_record_matches_item(
    item: &state::QueueItemRow,
    record: &state::TaskRecordRow,
) -> bool {
    queue_item_remote_native(item)
        && item.remote_launcher.is_some()
        && record.source == "task-flow"
        && task_record_remote_launcher(record).is_none()
        && queue_task_record_repo_matches_item(item, record)
        && record
            .agent_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn recovery_task_record_precedence(record: &state::TaskRecordRow) -> (u8, u64) {
    let rank = if record.status == "closed:success" {
        3
    } else if !queue_task_record_is_terminal(record) {
        2
    } else {
        1
    };
    (rank, record.updated_at)
}

fn related_recovery_task_record_id(task_id: &str, record_id: &str) -> bool {
    if record_id.starts_with(&format!("{task_id}-recovery")) {
        return true;
    }
    if record_id == task_id || !related_repair_task_marker(record_id) {
        return false;
    }
    let Some(task_slug) = task_id.strip_prefix("task/") else {
        return false;
    };
    let Some(record_slug) = record_id.strip_prefix("task/") else {
        return false;
    };
    let Some(family_prefix) = task_slug_family_prefix(task_slug) else {
        return false;
    };
    record_slug.starts_with(&family_prefix)
}

fn related_repair_task_marker(record_id: &str) -> bool {
    record_id
        .split(['/', '-'])
        .any(related_repair_task_marker_segment)
}

fn related_repair_task_marker_segment(segment: &str) -> bool {
    segment == "reintegrate"
        || retry_marker_segment(segment, "relaunch")
        || retry_marker_segment(segment, "repair")
}

fn retry_marker_segment(segment: &str, prefix: &str) -> bool {
    let Some(suffix) = segment.strip_prefix(prefix) else {
        return false;
    };
    suffix.is_empty() || suffix.chars().all(|ch| ch.is_ascii_digit())
}

fn task_slug_family_prefix(slug: &str) -> Option<String> {
    let parts = slug.split('-').take(4).collect::<Vec<_>>();
    if parts.len() < 4 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    Some(format!("{}-", parts.join("-")))
}

enum RemoteNativeSyncFallback {
    Status(String),
    PendingRecovery,
}

fn remote_native_sync_failure_fallback_status(
    item: &state::QueueItemRow,
    task_id: &str,
) -> Result<Option<RemoteNativeSyncFallback>> {
    let Some(record) = state::get_task_record(task_id)? else {
        return Ok(None);
    };
    if !queue_task_record_matches_item(item, &record) {
        return Ok(None);
    }
    if !queue_task_record_is_terminal(&record) {
        return Ok(Some(RemoteNativeSyncFallback::Status(record.status)));
    }
    if remote_native_failed_closeout_is_being_retried(item, &record.status) {
        return Ok(Some(RemoteNativeSyncFallback::PendingRecovery));
    }
    if queue_status_auto_recoverable(&record.status) && queue_item_recovery_active_or_pending(item)
    {
        return Ok(Some(RemoteNativeSyncFallback::PendingRecovery));
    }
    Ok(None)
}

fn remote_native_requires_task_record_sync(item: &state::QueueItemRow) -> bool {
    queue_item_remote_native(item) && item.status.is_starting_or_running()
}

fn remote_native_failed_closeout_is_being_retried(
    item: &state::QueueItemRow,
    status: &str,
) -> bool {
    status == "failed-closeout" && remote_native_retry_session_running(item)
}

fn remote_native_retry_session_running(item: &state::QueueItemRow) -> bool {
    queue_item_remote_native(item) && remote_native_open_record_live_agent_id(item).is_some()
}

fn agent_running(agent_id: &str) -> bool {
    running_agent_ids().contains(agent_id)
}

fn running_agent_ids() -> HashSet<String> {
    agents::running_snapshot().map_or_else(
        |_| HashSet::new(),
        |snapshot| {
            snapshot
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split('\t');
                    (fields.next() == Some("agent"))
                        .then(|| fields.next().map(str::to_string))
                        .flatten()
                })
                .collect()
        },
    )
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
