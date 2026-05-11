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

fn spawn_web_queue_worker(run_id: String) {
    let workers = WEB_QUEUE_WORKERS.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut active) = workers.lock() {
        if !active.insert(run_id.clone()) {
            return;
        }
    }
    thread::spawn(move || {
        if let Err(err) = run_web_queue(&run_id) {
            let _ = state::update_web_queue_run(&run_id, "failed", -1, &format!("{err:#}"));
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
                .is_some_and(|agent_id| !agent_running(agent_id)))
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
        if let Err(err) = run_web_queue_item(&run_id, &item) {
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
        if let Some(workers) = WEB_QUEUE_ITEM_WORKERS.get() {
            if let Ok(mut active) = workers.lock() {
                active.remove(&key);
            }
        }
    });
    true
}

fn queue_item_terminal(status: &str) -> bool {
    matches!(status, "success" | "failed" | "blocked")
}

enum QueueItemOutcome {
    Success,
    Stopped,
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
        if status.starts_with("closed") {
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
            if agent_running(agent_id) {
                return wait_for_queue_item_closeout(run_id, item, agent_id, item.attempts);
            }
            let message = "agent exited before task closeout";
            state::update_web_queue_item(
                run_id,
                &item.id,
                "failed",
                message,
                Some(agent_id),
                item.attempts,
                None,
            )?;
            return Ok(QueueItemOutcome::failed(message));
        }
    }
    if matches!(item.status.as_str(), "stopped" | "paused") {
        if let Some(agent_id) = item.agent_id.as_deref() {
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
        match start_web_queue_item(run_id, item, retries)? {
            QueueItemOutcome::Failed {
                message,
                retryable: true,
            } if retry_index(retries) < WEB_QUEUE_RETRY_DELAYS.len() =>
            {
                let delay = WEB_QUEUE_RETRY_DELAYS[retry_index(retries)];
                retries += 1;
                let next_attempt_at = unix_now().saturating_add(delay);
                let retry_message = format!(
                    "{message}; retry {}/{} in {}s",
                    retries,
                    WEB_QUEUE_RETRY_DELAYS.len(),
                    delay
                );
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "waiting",
                    &retry_message,
                    None,
                    retries,
                    Some(next_attempt_at),
                )?;
                if !sleep_queue_retry(run_id, delay)? {
                    return Ok(QueueItemOutcome::Stopped);
                }
            }
            outcome => return Ok(outcome),
        }
    }
}

fn start_web_queue_item(
    run_id: &str,
    item: &state::QueueItemRow,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    let request = AgentStartRequest {
        id: Some(queue_agent_id(item)),
        cwd: item.repo_root.as_ref().map(PathBuf::from),
        track: queue_track(run_id),
        command: item.agent_command.clone(),
    };
    let agent = match start_web_agent(&request) {
        Ok(agent) => agent,
        Err(err) => return Ok(QueueItemOutcome::retryable_failure(format!("{err:#}"))),
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
    let Some(target) = wait_for_agent_terminal_target(&agent.id) else {
        return Ok(retry_after_queue_agent_launch_failure(
            &agent.id,
            "agent terminal did not appear",
        ));
    };
    set_queue_terminal_scope(&target, item)?;
    thread::sleep(Duration::from_secs(1));
    if let Err(err) = send_terminal_text_to_target(&target, "/new") {
        return Ok(retry_after_queue_agent_launch_failure(
            &agent.id,
            &format!("{err:#}"),
        ));
    }
    thread::sleep(Duration::from_millis(500));
    if let Err(err) = send_terminal_text_to_target(&target, &queue_task_instruction(item)) {
        return Ok(retry_after_queue_agent_launch_failure(
            &agent.id,
            &format!("{err:#}"),
        ));
    }
    state::update_web_queue_item(
        run_id,
        &item.id,
        "running",
        &format!("{} ({})", queue_display_label(item), agent.id),
        Some(&agent.id),
        attempts,
        None,
    )?;
    wait_for_queue_item_closeout(run_id, item, &agent.id, attempts)
}

fn retry_after_queue_agent_launch_failure(agent_id: &str, message: &str) -> QueueItemOutcome {
    let cleanup = cleanup_queue_agent(agent_id);
    QueueItemOutcome::retryable_failure(format!("{message}; {cleanup}"))
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
            if status.starts_with("closed") {
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
            if status == "open" && !agent_running(agent_id) {
                let message = "agent exited before task closeout".to_string();
                state::update_web_queue_item(
                    run_id,
                    &item.id,
                    "failed",
                    &message,
                    Some(agent_id),
                    attempts,
                    None,
                )?;
                return Ok(QueueItemOutcome::failed(message));
            }
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
        |agent_id| format!("closed successfully; {}", cleanup_queue_agent(agent_id)),
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

fn cleanup_queue_agent(agent_id: &str) -> String {
    match agents::terminate_agent(agent_id) {
        Ok(true) => "agent terminal closed".to_string(),
        Ok(false) => "agent already stopped".to_string(),
        Err(err) => format!("agent cleanup failed: {err:#}"),
    }
}

fn reconcile_stale_web_queue_run() -> Result<()> {
    let (run, items) = state::load_web_queue()?;
    let Some(run) = run else {
        return Ok(());
    };
    if !matches!(
        run.status.as_str(),
        "running" | "waiting" | "starting" | "stopping"
    ) || web_queue_worker_active(&run.id)
    {
        return Ok(());
    }

    crate::sync_codex_task_records().ok();
    cleanup_orphaned_queue_agents(&run, &items);
    for item in items {
        if let Some(status) = queue_task_status(&item)? {
            if status == "closed:success" {
                if item.status != "success"
                    || item
                        .agent_id
                        .as_deref()
                        .is_some_and(agent_running)
                {
                    update_successful_queue_item(
                        &run.id,
                        &item,
                        item.agent_id.as_deref(),
                        item.attempts,
                    )?;
                }
                continue;
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
                return Ok(());
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
                return Ok(());
            }
        }

        if item.status == "success" {
            if item
                .agent_id
                .as_deref()
                .is_some_and(agent_running)
            {
                update_successful_queue_item(
                    &run.id,
                    &item,
                    item.agent_id.as_deref(),
                    item.attempts,
                )?;
            }
            continue;
        }
        if let Some(agent_id) = item.agent_id.as_deref() {
            if matches!(item.status.as_str(), "running" | "starting") && !agent_running(agent_id) {
                let message = "agent exited before task closeout";
                state::update_web_queue_item(
                    &run.id,
                    &item.id,
                    "failed",
                    message,
                    Some(agent_id),
                    item.attempts,
                    None,
                )?;
                state::update_web_queue_run(&run.id, "failed", item.position, message)?;
                return Ok(());
            }
        }
        state::update_web_queue_run(
            &run.id,
            "running",
            item.position,
            &format!("running {}", item.slug),
        )?;
        spawn_web_queue_worker(run.id);
        return Ok(());
    }

    state::update_web_queue_run(&run.id, "success", -1, "closed successfully")?;
    Ok(())
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
    let Some(record) = state::get_task_record(&task_id)? else {
        return Ok(None);
    };
    if item
        .repo_root
        .as_deref()
        .is_some_and(|repo| record.repo_root.as_deref().is_some_and(|value| value != repo))
    {
        return Ok(None);
    }
    Ok(Some(record.status))
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

fn send_terminal_text_to_target(target: &str, text: &str) -> Result<()> {
    send_terminal_paste(target, text, true)
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

fn queue_task_instruction(item: &state::QueueItemRow) -> String {
    let root = item.repo_root.as_deref().unwrap_or("<repo>");
    let mut packet = String::new();
    let _ = writeln!(packet, "Q-COLD_TASK_PACKET");
    let _ = writeln!(packet, "repo_root: {root}");
    let _ = writeln!(packet, "task_slug: {}", item.slug);
    let _ = writeln!(packet, "selected_command: {}", item.agent_command);
    let _ = writeln!(packet, "launch_context: host-side $QCOLD_AGENT_WORKTREE");
    let _ = writeln!(packet, "required_flow:");
    let _ = writeln!(packet, "  - cargo qcold task open {}", item.slug);
    let _ = writeln!(packet, "  - cargo qcold task enter");
    let _ = writeln!(packet, "  - reread AGENTS.md and available task logs");
    let _ = writeln!(packet, "state_pointers:");
    let _ = writeln!(packet, "  task_env: .task/task.env (after open, if present)");
    let _ = writeln!(packet, "  task_logs: .task/logs/ (after open, if present)");
    let _ = writeln!(packet, "validation_closeout:");
    let _ = writeln!(packet, "  expect: run relevant validation, then terminal closeout");
    let _ = writeln!(
        packet,
        "  success: cargo qcold task closeout --outcome success --message \"<message>\""
    );
    let _ = writeln!(packet, "blocker_boundary:");
    let _ = writeln!(packet, "  pause_or_blocked_only_for: business decision or external resource");
    let _ = writeln!(packet, "operator_request: |");
    for line in item.prompt.trim().lines() {
        let _ = writeln!(packet, "  {line}");
    }
    let _ = writeln!(packet, "after_closeout: cd back to $QCOLD_AGENT_WORKTREE");
    let _ = writeln!(packet, "END_Q-COLD_TASK_PACKET");
    packet
}

fn clean_queue_run_id(value: &str) -> String {
    sanitize_daemon_id(value)
}

fn clean_queue_slug(
    value: &str,
    run_id: &str,
    index: usize,
    used_slugs: &mut HashSet<String>,
) -> String {
    let mut slug = sanitize_daemon_id(value);
    if slug.is_empty() {
        slug = queue_slug(run_id, index);
    }
    while !used_slugs.insert(slug.clone()) {
        slug = queue_slug(run_id, used_slugs.len());
    }
    slug
}

fn queue_track(run_id: &str) -> String {
    format!("queue-{}", sanitize_daemon_id(run_id))
}

fn queue_agent_id(item: &state::QueueItemRow) -> String {
    let slug = sanitize_daemon_id(&item.slug);
    if slug.len() <= 36 {
        format!("qa-{slug}")
    } else {
        let prefix = slug.chars().take(24).collect::<String>();
        format!("qa-{prefix}-{}", stable_short_hash(&item.id))
    }
}

fn queue_slug(run_id: &str, index: usize) -> String {
    format!("task-{}-{:02}", sanitize_daemon_id(run_id), index + 1)
}
