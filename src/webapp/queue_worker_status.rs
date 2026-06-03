fn queue_task_status(item: &state::QueueItemRow) -> Result<Option<String>> {
    let task_id = format!("task/{}", item.slug);
    if item.remote_launcher.is_some() {
        let required_remote_native_sync = remote_native_requires_task_record_sync(item);
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
    let record = match state::get_task_record(&task_id)? {
        Some(record) if queue_task_record_matches_item(item, &record) => Some(record),
        Some(_) => return Ok(None),
        None => None,
    };
    queue_task_status_from_record_or_recovery(item, &task_id, record.as_ref())
}

fn queue_task_status_from_record_or_recovery(
    item: &state::QueueItemRow,
    task_id: &str,
    record: Option<&state::TaskRecordRow>,
) -> Result<Option<String>> {
    let recovery = latest_related_recovery_task_record(item, task_id)?;
    if let Some(recovery) = recovery.as_ref() {
        if record.is_none_or(|record| recovery.updated_at >= record.updated_at) {
            if remote_native_failed_closeout_is_being_retried(item, &recovery.status) {
                return Ok(None);
            }
            return Ok(Some(recovery.status.clone()));
        }
    }
    if record.is_some_and(|record| {
        remote_native_failed_closeout_is_being_retried(item, &record.status)
    }) {
        return Ok(None);
    }
    Ok(record.map(|record| record.status.clone()))
}

fn latest_related_recovery_task_record(
    item: &state::QueueItemRow,
    task_id: &str,
) -> Result<Option<state::TaskRecordRow>> {
    let Some(repo_root) = item.repo_root.as_deref().filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let recovery_prefix = format!("{task_id}-recovery");
    let records = state::load_task_records_for_repo(repo_root, None, 128)?;
    Ok(records
        .into_iter()
        .filter(|record| {
            record.id.starts_with(&recovery_prefix)
                && queue_task_record_matches_item(item, record)
                && (item.started_at == 0 || record.updated_at >= item.started_at)
        })
        .max_by_key(|record| record.updated_at))
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
    queue_item_remote_native(item) && matches!(item.status.as_str(), "starting" | "running")
}

fn remote_native_failed_closeout_is_being_retried(
    item: &state::QueueItemRow,
    status: &str,
) -> bool {
    status == "failed-closeout" && remote_native_retry_session_running(item)
}

fn remote_native_retry_session_running(item: &state::QueueItemRow) -> bool {
    queue_item_remote_native(item)
        && item
            .agent_id
            .as_deref()
            .is_some_and(|agent_id| remote_native_session_running(item, agent_id))
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
