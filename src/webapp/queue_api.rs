fn handle_queue_run(headers: &HeaderMap, payload: QueueRunRequest) -> TerminalSendResponse {
    match handle_queue_run_result(headers, payload) {
        Ok(run_id) => TerminalSendResponse {
            ok: true,
            output: format!("queue-run\t{run_id}"),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_run_result(headers: &HeaderMap, payload: QueueRunRequest) -> Result<String> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let selected_agent_command = payload.selected_agent_command.trim();
    if selected_agent_command.is_empty() {
        bail!("queue agent command is empty");
    }
    if !agents::available_agent_commands()
        .iter()
        .any(|agent| agent.command == selected_agent_command)
    {
        bail!("unknown queue agent command: {selected_agent_command}");
    }
    let prompts = payload
        .items
        .into_iter()
        .filter(|item| !item.prompt.trim().is_empty())
        .collect::<Vec<_>>();
    if prompts.is_empty() {
        bail!("queue has no runnable items");
    }
    let fallback_run_id = base36_time_id();
    let run_id = clean_queue_run_id(
        payload
            .run_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&fallback_run_id),
    );
    let now = unix_now();
    let track = queue_track(&run_id);
    let execution_mode = clean_queue_execution_mode(payload.execution_mode.as_deref());
    let run = state::QueueRunRow {
        id: run_id.clone(),
        status: "running".to_string(),
        execution_mode,
        selected_agent_command: selected_agent_command.to_string(),
        selected_repo_root: payload.selected_repo_root.filter(|value| !value.trim().is_empty()),
        selected_repo_name: payload.selected_repo_name.filter(|value| !value.trim().is_empty()),
        track,
        current_index: -1,
        stop_requested: false,
        message: "queued".to_string(),
        created_at: now,
        updated_at: now,
    };
    let mut used_slugs = HashSet::new();
    let mut items = prompts
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let fallback_slug = queue_slug(&run_id, index);
            let slug = clean_queue_slug(
                item.slug
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(&fallback_slug),
                &run_id,
                index,
                &mut used_slugs,
            );
            state::QueueItemRow {
                id: item
                    .id
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| format!("queue-{run_id}-{}", index + 1)),
                run_id: run_id.clone(),
                position: i64::try_from(index).unwrap_or(i64::MAX),
                depends_on: item.depends_on.unwrap_or_default(),
                prompt: item.prompt.trim().to_string(),
                slug,
                repo_root: item
                    .repo_root
                    .or_else(|| run.selected_repo_root.clone())
                    .filter(|value| !value.trim().is_empty()),
                repo_name: item
                    .repo_name
                    .or_else(|| run.selected_repo_name.clone())
                    .filter(|value| !value.trim().is_empty()),
                agent_command: item
                    .agent_command
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| selected_agent_command.to_string()),
                agent_id: None,
                status: "pending".to_string(),
                message: String::new(),
                attempts: 0,
                next_attempt_at: None,
                started_at: now,
                updated_at: now,
            }
        })
        .collect::<Vec<_>>();
    normalize_queue_dependencies(&run.execution_mode, &mut items)?;
    state::replace_web_queue(&run, &items)?;
    spawn_web_queue_worker(run_id.clone());
    Ok(run_id)
}

fn handle_queue_append(headers: &HeaderMap, payload: QueueAppendRequest) -> TerminalSendResponse {
    match handle_queue_append_result(headers, payload) {
        Ok(count) => TerminalSendResponse {
            ok: true,
            output: format!("appended {count} queue item(s)"),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_append_result(headers: &HeaderMap, payload: QueueAppendRequest) -> Result<usize> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let run_id = clean_queue_run_id(&payload.run_id);
    let (run, existing_items) = state::load_web_queue_run(&run_id)?;
    let Some(run) = run else {
        bail!("unknown queue run: {run_id}");
    };
    if !matches!(
        run.status.as_str(),
        "running" | "waiting" | "starting" | "stopped"
    ) {
        bail!("queue is not appendable");
    }
    let prompts = payload
        .items
        .into_iter()
        .filter(|item| !item.prompt.trim().is_empty())
        .collect::<Vec<_>>();
    if prompts.is_empty() {
        bail!("queue append has no runnable items");
    }
    let now = unix_now();
    let mut used_slugs = existing_items
        .iter()
        .map(|item| item.slug.clone())
        .collect::<HashSet<_>>();
    let start_position = existing_items
        .iter()
        .map(|item| item.position)
        .max()
        .unwrap_or(-1)
        .saturating_add(1);
    let mut items = prompts
        .into_iter()
        .enumerate()
        .map(|(offset, item)| {
            let index = usize::try_from(start_position)
                .unwrap_or(0)
                .saturating_add(offset);
            let fallback_slug = queue_slug(&run_id, index);
            let slug = clean_queue_slug(
                item.slug
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(&fallback_slug),
                &run_id,
                index,
                &mut used_slugs,
            );
            state::QueueItemRow {
                id: item
                    .id
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| format!("queue-{run_id}-{}", index + 1)),
                run_id: run_id.clone(),
                position: start_position.saturating_add(i64::try_from(offset).unwrap_or(0)),
                depends_on: item.depends_on.unwrap_or_default(),
                prompt: item.prompt.trim().to_string(),
                slug,
                repo_root: item
                    .repo_root
                    .or_else(|| run.selected_repo_root.clone())
                    .filter(|value| !value.trim().is_empty()),
                repo_name: item
                    .repo_name
                    .or_else(|| run.selected_repo_name.clone())
                    .filter(|value| !value.trim().is_empty()),
                agent_command: item
                    .agent_command
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| run.selected_agent_command.clone()),
                agent_id: None,
                status: "pending".to_string(),
                message: String::new(),
                attempts: 0,
                next_attempt_at: None,
                started_at: now,
                updated_at: now,
            }
        })
        .collect::<Vec<_>>();
    let mut all_items = existing_items;
    all_items.extend(items.clone());
    normalize_queue_dependencies(&run.execution_mode, &mut all_items)?;
    let normalized = all_items
        .into_iter()
        .filter(|item| items.iter().any(|new_item| new_item.id == item.id))
        .collect::<Vec<_>>();
    items = normalized;
    let count = items.len();
    state::append_web_queue_items(&run_id, &items)?;
    spawn_web_queue_worker(run_id);
    Ok(count)
}

fn handle_queue_update(headers: &HeaderMap, payload: QueueUpdateRequest) -> TerminalSendResponse {
    match handle_queue_update_result(headers, payload) {
        Ok(count) => TerminalSendResponse {
            ok: true,
            output: format!("updated {count} queue item(s)"),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_update_result(headers: &HeaderMap, payload: QueueUpdateRequest) -> Result<usize> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let run_id = clean_queue_run_id(&payload.run_id);
    let (run, existing_items) = state::load_web_queue_run(&run_id)?;
    let Some(run) = run else {
        bail!("unknown queue run: {run_id}");
    };
    if !matches!(
        run.status.as_str(),
        "running" | "waiting" | "starting" | "stopped"
    ) {
        bail!("queue is not editable");
    }
    let requested = payload
        .items
        .into_iter()
        .filter(|item| !item.id.trim().is_empty())
        .map(|item| (item.id.clone(), item))
        .collect::<HashMap<_, _>>();
    if requested.is_empty() {
        bail!("queue update has no items");
    }
    let mut all_items = existing_items;
    let mut updated_ids = HashSet::new();
    for item in &mut all_items {
        let Some(request) = requested.get(&item.id) else {
            continue;
        };
        if !queue_item_editable_while_running(&run, item)? {
            bail!("queue item is already active: {}", item.id);
        }
        if request.position.is_some_and(|position| position <= run.current_index) {
            bail!("queue item cannot move before the active cursor: {}", item.id);
        }
        let prompt = request.prompt.trim();
        if prompt.is_empty() {
            bail!("queue item prompt is empty: {}", item.id);
        }
        item.prompt = prompt.to_string();
        item.position = request.position.unwrap_or(item.position);
        item.depends_on = request.depends_on.clone().unwrap_or_default();
        item.repo_root = request
            .repo_root
            .clone()
            .or_else(|| run.selected_repo_root.clone())
            .filter(|value| !value.trim().is_empty());
        item.repo_name = request
            .repo_name
            .clone()
            .or_else(|| run.selected_repo_name.clone())
            .filter(|value| !value.trim().is_empty());
        item.agent_command = request
            .agent_command
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| run.selected_agent_command.clone());
        updated_ids.insert(item.id.clone());
    }
    if updated_ids.len() != requested.len() {
        bail!("queue update references unknown item");
    }
    normalize_queue_dependencies(&run.execution_mode, &mut all_items)?;
    let updates = all_items
        .into_iter()
        .filter(|item| updated_ids.contains(&item.id))
        .collect::<Vec<_>>();
    let count = updates.len();
    state::update_web_queue_item_plans(&run_id, &updates)?;
    spawn_web_queue_worker(run_id);
    Ok(count)
}

fn handle_queue_stop(headers: &HeaderMap) -> TerminalSendResponse {
    match handle_queue_stop_result(headers) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "queue stop requested".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_stop_result(headers: &HeaderMap) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    state::request_web_queue_stop()
}

fn handle_queue_continue(
    headers: &HeaderMap,
    payload: &QueueContinueRequest,
) -> TerminalSendResponse {
    match handle_queue_continue_result(headers, payload) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "queue continued".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_continue_result(
    headers: &HeaderMap,
    payload: &QueueContinueRequest,
) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let run_id = clean_queue_run_id(&payload.run_id);
    state::continue_web_queue_run(&run_id)?;
    spawn_web_queue_worker(run_id);
    Ok(())
}

fn clean_queue_execution_mode(value: Option<&str>) -> String {
    match value.map(str::trim) {
        Some("graph") => "graph".to_string(),
        _ => "sequence".to_string(),
    }
}

fn normalize_queue_dependencies(
    execution_mode: &str,
    items: &mut [state::QueueItemRow],
) -> Result<()> {
    if execution_mode != "graph" {
        for item in items {
            item.depends_on.clear();
        }
        return Ok(());
    }
    let ids = items
        .iter()
        .map(|item| item.id.clone())
        .collect::<HashSet<_>>();
    for item in items.iter_mut() {
        let mut seen = HashSet::new();
        item.depends_on.retain(|dependency| {
            dependency != &item.id && ids.contains(dependency) && seen.insert(dependency.clone())
        });
    }
    if queue_dependency_graph_has_cycle(items) {
        bail!("queue dependency graph contains a cycle");
    }
    Ok(())
}

fn queue_dependency_graph_has_cycle(items: &[state::QueueItemRow]) -> bool {
    let by_id = items
        .iter()
        .map(|item| (item.id.as_str(), item.depends_on.as_slice()))
        .collect::<HashMap<_, _>>();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    items
        .iter()
        .any(|item| queue_dependency_visit(&by_id, item.id.as_str(), &mut visiting, &mut visited))
}

fn queue_dependency_visit<'a>(
    by_id: &HashMap<&'a str, &'a [String]>,
    id: &'a str,
    visiting: &mut HashSet<&'a str>,
    visited: &mut HashSet<&'a str>,
) -> bool {
    if visited.contains(id) {
        return false;
    }
    if !visiting.insert(id) {
        return true;
    }
    if let Some(dependencies) = by_id.get(id) {
        for dependency in *dependencies {
            if queue_dependency_visit(by_id, dependency.as_str(), visiting, visited) {
                return true;
            }
        }
    }
    visiting.remove(id);
    visited.insert(id);
    false
}

fn handle_queue_remove(headers: &HeaderMap, payload: &QueueRemoveRequest) -> TerminalSendResponse {
    match handle_queue_remove_result(headers, payload) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "removed".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_remove_result(headers: &HeaderMap, payload: &QueueRemoveRequest) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let run_id = clean_queue_run_id(&payload.run_id);
    let item_id = payload.item_id.trim();
    if item_id.is_empty() || item_id.chars().any(char::is_control) {
        bail!("invalid queue item id");
    }
    let (run, items) = state::load_web_queue_run(&run_id)?;
    let item = items.iter().find(|item| item.id == item_id);
    if let Some(run) = run.as_ref().filter(|run| {
        matches!(
            run.status.as_str(),
            "running" | "waiting" | "starting" | "stopping"
        )
    }) {
        let Some(item) = item else {
            return Ok(());
        };
        if !queue_item_removable_while_running(run, item)? {
            bail!("cannot remove active queue items while the queue is running");
        }
    }
    let task_id = payload
        .task_id
        .as_deref()
        .filter(|id| !id.trim().is_empty());
    let agent_id = payload
        .agent_id
        .as_deref()
        .filter(|id| !id.trim().is_empty());
    match state::delete_web_queue_item_if_exists(&run_id, item_id)? {
        Some(item) => cleanup_queue_item_artifacts(&item, task_id, agent_id),
        None => cleanup_task_agent_artifacts(task_id, agent_id),
    }
}

fn queue_item_removable_while_running(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
) -> Result<bool> {
    if queue_item_terminal(&item.status) {
        return Ok(true);
    }
    if matches!(item.status.as_str(), "pending" | "waiting")
        && item.agent_id.is_none()
        && item.position > run.current_index
    {
        return Ok(true);
    }
    Ok(queue_task_status(item)?.is_some_and(|status| status.starts_with("closed")))
}

fn queue_item_editable_while_running(
    run: &state::QueueRunRow,
    item: &state::QueueItemRow,
) -> Result<bool> {
    if !matches!(item.status.as_str(), "pending" | "waiting") {
        return Ok(false);
    }
    if item.agent_id.is_some() || queue_item_worker_active(&run.id, &item.id) {
        return Ok(false);
    }
    if item.position <= run.current_index {
        return Ok(false);
    }
    Ok(!queue_task_status(item)?.is_some_and(|status| {
        status == "paused" || status.starts_with("closed")
    }))
}

fn handle_queue_clear(headers: &HeaderMap, payload: &QueueClearRequest) -> TerminalSendResponse {
    match handle_queue_clear_result(headers, payload) {
        Ok(count) => TerminalSendResponse {
            ok: true,
            output: format!("cleared {count} queue item(s)"),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_clear_result(headers: &HeaderMap, payload: &QueueClearRequest) -> Result<usize> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let requested_run_id = payload
        .run_id
        .as_deref()
        .map(clean_queue_run_id)
        .filter(|run_id| !run_id.is_empty());
    let (run, items) = match requested_run_id {
        Some(run_id) => state::load_web_queue_run(&run_id)?,
        None => state::load_web_queue()?,
    };
    let Some(run) = run else {
        return Ok(0);
    };
    if matches!(
        run.status.as_str(),
        "running" | "waiting" | "starting" | "stopping"
    ) {
        state::request_web_queue_stop()?;
    }
    let mut removed = 0;
    for item in items {
        let item = state::delete_web_queue_item(&run.id, &item.id)?;
        cleanup_queue_item_artifacts(&item, None, None)?;
        removed += 1;
    }
    state::delete_empty_web_queue_run(&run.id)?;
    Ok(removed)
}

fn cleanup_queue_item_artifacts(
    item: &state::QueueItemRow,
    task_id: Option<&str>,
    agent_id: Option<&str>,
) -> Result<()> {
    let default_task_id = format!("task/{}", item.slug);
    let task_id = task_id
        .filter(|id| !id.trim().is_empty())
        .map_or(default_task_id, str::to_string);
    let task = state::get_task_record(&task_id)?;
    let agent_id = agent_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .or_else(|| item.agent_id.clone())
        .or_else(|| task.as_ref().and_then(|task| task.agent_id.clone()));
    cleanup_existing_task_agent_artifacts(&task_id, task.as_ref(), agent_id)
}

fn cleanup_task_agent_artifacts(task_id: Option<&str>, agent_id: Option<&str>) -> Result<()> {
    let task_id = task_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string);
    let task = task_id
        .as_deref()
        .map(state::get_task_record)
        .transpose()?
        .flatten();
    let agent_id = agent_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .or_else(|| task.as_ref().and_then(|task| task.agent_id.clone()));
    if let Some(task_id) = task_id {
        cleanup_existing_task_agent_artifacts(&task_id, task.as_ref(), agent_id)?;
    } else if let Some(agent_id) = agent_id {
        let _ = agents::terminate_agent(&agent_id);
    }
    Ok(())
}
