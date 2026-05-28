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
    let selected_agent_command = payload.selected_agent_command.trim().to_string();
    if selected_agent_command.is_empty() {
        bail!("queue agent command is empty");
    }
    let run_execution_host = resolve_queue_execution_host(payload.selected_execution_host.as_deref());
    let has_local_item = payload.items.iter().any(|item| {
        !item.prompt.trim().is_empty()
            && resolve_queue_item_execution_host(
                item.execution_host.as_deref(),
                None,
                run_execution_host.clone(),
            ) == "local"
    });
    if has_local_item
        && !agents::available_agent_commands()
        .iter()
        .any(|agent| agent.command == selected_agent_command)
    {
        bail!("unknown queue agent command: {selected_agent_command}");
    }
    let fallback_run_id = base36_time_id();
    let run_id = clean_queue_run_id(
        payload
            .run_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&fallback_run_id),
    );
    let tab_id = payload
        .tab_id
        .as_deref()
        .map(clean_queue_tab_id)
        .filter(|value| !value.is_empty());
    let execution_mode = clean_queue_execution_mode(payload.execution_mode.as_deref());
    let now = unix_now();
    let run = queue_run_from_request(&run_id, &payload, &selected_agent_command, execution_mode, now);
    let prompts = payload
        .items
        .into_iter()
        .filter(|item| !item.prompt.trim().is_empty())
        .collect::<Vec<_>>();
    if prompts.is_empty() {
        bail!("queue has no runnable items");
    }
    let mut used_slugs = HashSet::new();
    let mut items = queue_items_from_requests(&run, prompts, 0, &mut used_slugs, now);
    validate_queue_execution_hosts(&run, &items)?;
    normalize_queue_dependencies(&run.execution_mode, &mut items)?;
    if let Some(tab_id) = tab_id.as_deref() {
        state::replace_web_queue_for_tab(tab_id, &run, &items)?;
    } else {
        state::replace_web_queue(&run, &items)?;
    }
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
    let mut items = queue_items_from_requests(&run, prompts, start_position, &mut used_slugs, now);
    validate_queue_execution_hosts(&run, &items)?;
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
        item.execution_host = resolve_queue_item_execution_host(
            request.execution_host.as_deref(),
            Some(item.execution_host.clone()),
            run.execution_host.clone(),
        );
        item.remote_launcher = update_queue_item_remote_launcher(
            request.remote_launcher.as_deref(),
            item.remote_launcher.clone(),
            run.remote_launcher.clone(),
            item.repo_root.as_deref(),
        );
        item.remote_agent_local_proxy = resolve_queue_item_optional_setting(
            request.remote_agent_local_proxy.as_deref(),
            item.remote_agent_local_proxy.clone(),
            run.remote_agent_local_proxy.clone(),
        );
        item.remote_agent_remote_proxy = resolve_queue_item_optional_setting(
            request.remote_agent_remote_proxy.as_deref(),
            item.remote_agent_remote_proxy.clone(),
            run.remote_agent_remote_proxy.clone(),
        );
        updated_ids.insert(item.id.clone());
    }
    if updated_ids.len() != requested.len() {
        bail!("queue update references unknown item");
    }
    validate_queue_execution_hosts(&run, &all_items)?;
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

fn handle_queue_stop(headers: &HeaderMap, payload: &QueueStopRequest) -> TerminalSendResponse {
    match handle_queue_stop_result(headers, payload) {
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

fn handle_queue_stop_result(headers: &HeaderMap, payload: &QueueStopRequest) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let run_id = payload
        .run_id
        .as_deref()
        .map(clean_queue_run_id)
        .filter(|run_id| !run_id.is_empty());
    state::request_web_queue_stop(run_id.as_deref())
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

fn handle_queue_tab_create(
    headers: &HeaderMap,
    payload: &QueueTabCreateRequest,
) -> TerminalSendResponse {
    match handle_queue_tab_create_result(headers, payload) {
        Ok(tab_id) => TerminalSendResponse {
            ok: true,
            output: format!("queue-tab\t{tab_id}"),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_tab_create_result(
    headers: &HeaderMap,
    payload: &QueueTabCreateRequest,
) -> Result<String> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let fallback = format!("queue-{}", base36_time_id());
    let raw_label = payload
        .label
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Queue");
    let tab_id = unique_queue_tab_id(&fallback)?;
    let label = clean_queue_tab_label(raw_label);
    state::create_web_queue_tab(&tab_id, &label)?;
    Ok(tab_id)
}

fn handle_queue_tab_switch(
    headers: &HeaderMap,
    payload: &QueueTabRequest,
) -> TerminalSendResponse {
    match handle_queue_tab_switch_result(headers, payload) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "queue tab switched".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_tab_switch_result(headers: &HeaderMap, payload: &QueueTabRequest) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let tab_id = clean_queue_tab_id(&payload.tab_id);
    if tab_id.is_empty() {
        bail!("invalid queue tab id");
    }
    state::activate_web_queue_tab(&tab_id)
}

fn handle_queue_tab_delete(
    headers: &HeaderMap,
    payload: &QueueTabRequest,
) -> TerminalSendResponse {
    match handle_queue_tab_delete_result(headers, payload) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "queue tab deleted".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_queue_tab_delete_result(headers: &HeaderMap, payload: &QueueTabRequest) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let tab_id = clean_queue_tab_id(&payload.tab_id);
    let tab = state::load_web_queue_tab(&tab_id)?
        .with_context(|| format!("unknown queue tab: {tab_id}"))?;
    if tab.is_default {
        bail!("cannot delete the default queue tab");
    }
    if let Some(run_id) = tab.run_id.as_deref() {
        let (run, items) = state::load_web_queue_run(run_id)?;
        let Some(run) = run else {
            state::delete_web_queue_tab(&tab_id)?;
            return Ok(());
        };
        if queue_run_has_live_work(&run, &items) {
            bail!("cannot delete a queue tab while it has running work");
        }
        for item in items {
            let item = state::delete_web_queue_item(&run.id, &item.id)?;
            cleanup_queue_item_artifacts(&item, None, None)?;
        }
        state::delete_empty_web_queue_run(&run.id)?;
    }
    state::delete_web_queue_tab(&tab_id)
}

fn queue_run_has_live_work(run: &state::QueueRunRow, items: &[state::QueueItemRow]) -> bool {
    if matches!(
        run.status.as_str(),
        "running" | "waiting" | "starting" | "stopping"
    ) {
        return true;
    }
    items.iter().any(|item| {
        matches!(item.status.as_str(), "running" | "starting" | "waiting")
            || item.agent_id.as_deref().is_some_and(agent_running)
    })
}

fn clean_queue_execution_mode(value: Option<&str>) -> String {
    match value.map(str::trim) {
        Some("graph") => "graph".to_string(),
        _ => "sequence".to_string(),
    }
}

fn clean_queue_tab_id(value: &str) -> String {
    sanitize_daemon_id(value)
}

fn clean_queue_tab_label(value: &str) -> String {
    let label = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(48)
        .collect::<String>();
    if label.is_empty() {
        "Queue".to_string()
    } else {
        label
    }
}

fn unique_queue_tab_id(prefix: &str) -> Result<String> {
    let base = clean_queue_tab_id(prefix);
    if base.is_empty() {
        bail!("invalid queue tab id");
    }
    let existing = state::load_web_queue_tabs()?
        .into_iter()
        .map(|tab| tab.id)
        .collect::<HashSet<_>>();
    if !existing.contains(&base) {
        return Ok(base);
    }
    for index in 2..100 {
        let candidate = format!("{base}-{index}");
        if !existing.contains(&candidate) {
            return Ok(candidate);
        }
    }
    bail!("failed to allocate queue tab id")
}

fn queue_run_from_request(
    run_id: &str,
    payload: &QueueRunRequest,
    selected_agent_command: &str,
    execution_mode: String,
    now: u64,
) -> state::QueueRunRow {
    let selected_repo_root = payload
        .selected_repo_root
        .clone()
        .filter(|value| !value.trim().is_empty());
    let selected_repo_name = payload
        .selected_repo_name
        .clone()
        .filter(|value| !value.trim().is_empty());
    let remote_launcher = resolve_queue_remote_launcher(
        payload.selected_remote_launcher.as_deref(),
        selected_repo_root.as_deref(),
    );
    let execution_host = resolve_queue_execution_host(payload.selected_execution_host.as_deref());
    state::QueueRunRow {
        id: run_id.to_string(),
        status: "running".to_string(),
        execution_mode,
        execution_host,
        selected_agent_command: selected_agent_command.to_string(),
        remote_launcher,
        remote_agent_local_proxy: resolve_queue_remote_agent_proxy(
            payload.selected_remote_agent_local_proxy.as_deref(),
            "QCOLD_QUEUE_REMOTE_AGENT_LOCAL_PROXY",
        ),
        remote_agent_remote_proxy: resolve_queue_remote_agent_proxy(
            payload.selected_remote_agent_remote_proxy.as_deref(),
            "QCOLD_QUEUE_REMOTE_AGENT_REMOTE_PROXY",
        ),
        selected_repo_root,
        selected_repo_name,
        track: queue_track(run_id),
        current_index: -1,
        stop_requested: false,
        message: "queued".to_string(),
        created_at: now,
        updated_at: now,
    }
}

fn queue_items_from_requests(
    run: &state::QueueRunRow,
    requests: Vec<QueueRunItemRequest>,
    start_position: i64,
    used_slugs: &mut HashSet<String>,
    now: u64,
) -> Vec<state::QueueItemRow> {
    requests
        .into_iter()
        .enumerate()
        .map(|(offset, request)| {
            let position = start_position.saturating_add(i64::try_from(offset).unwrap_or(0));
            queue_item_from_request(run, request, position, used_slugs, now)
        })
        .collect()
}

fn queue_item_from_request(
    run: &state::QueueRunRow,
    request: QueueRunItemRequest,
    position: i64,
    used_slugs: &mut HashSet<String>,
    now: u64,
) -> state::QueueItemRow {
    let index = usize::try_from(position).unwrap_or(usize::MAX);
    let fallback_slug = queue_slug(&run.id, index);
    let slug = clean_queue_slug(
        request
            .slug
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&fallback_slug),
        &run.id,
        index,
        used_slugs,
    );
    let repo_root = request
        .repo_root
        .or_else(|| run.selected_repo_root.clone())
        .filter(|value| !value.trim().is_empty());
    let repo_name = request
        .repo_name
        .or_else(|| run.selected_repo_name.clone())
        .filter(|value| !value.trim().is_empty());
    let remote_launcher = resolve_queue_item_remote_launcher(
        request.remote_launcher.as_deref(),
        run.remote_launcher.clone(),
        repo_root.as_deref(),
    );
    let execution_host = resolve_queue_item_execution_host(
        request.execution_host.as_deref(),
        None,
        run.execution_host.clone(),
    );
    state::QueueItemRow {
        id: request
            .id
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("queue-{}-{}", run.id, position.saturating_add(1))),
        run_id: run.id.clone(),
        position,
        depends_on: request.depends_on.unwrap_or_default(),
        prompt: request.prompt.trim().to_string(),
        slug,
        repo_root,
        repo_name,
        execution_host,
        agent_command: request
            .agent_command
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| run.selected_agent_command.clone()),
        remote_launcher,
        remote_agent_local_proxy: resolve_queue_item_optional_setting(
            request.remote_agent_local_proxy.as_deref(),
            None,
            run.remote_agent_local_proxy.clone(),
        ),
        remote_agent_remote_proxy: resolve_queue_item_optional_setting(
            request.remote_agent_remote_proxy.as_deref(),
            None,
            run.remote_agent_remote_proxy.clone(),
        ),
        agent_id: None,
        status: "pending".to_string(),
        message: String::new(),
        attempts: 0,
        next_attempt_at: None,
        started_at: now,
        updated_at: now,
    }
}

fn resolve_queue_execution_host(requested: Option<&str>) -> String {
    if let Some(host) = queue_execution_host_setting(requested) {
        return host;
    }
    let env_host = env::var("QCOLD_QUEUE_EXECUTION_HOST").ok();
    queue_execution_host_setting(env_host.as_deref()).unwrap_or_else(|| "local".to_string())
}

fn resolve_queue_item_execution_host(
    requested: Option<&str>,
    current: Option<String>,
    inherited: String,
) -> String {
    queue_execution_host_setting(requested)
        .or(current)
        .unwrap_or(inherited)
}

fn resolve_queue_remote_launcher(requested: Option<&str>, _repo_root: Option<&str>) -> Option<String> {
    if let Some(setting) = queue_remote_launcher_setting(requested) {
        return setting.into_launcher();
    }
    let env_launcher = env::var("QCOLD_QUEUE_REMOTE_LAUNCHER").ok();
    if let Some(setting) = queue_remote_launcher_setting(env_launcher.as_deref()) {
        return setting.into_launcher();
    }
    None
}

fn resolve_queue_item_remote_launcher(
    requested: Option<&str>,
    inherited: Option<String>,
    _repo_root: Option<&str>,
) -> Option<String> {
    if let Some(setting) = queue_remote_launcher_setting(requested) {
        return setting.into_launcher();
    }
    inherited
}

fn update_queue_item_remote_launcher(
    requested: Option<&str>,
    current: Option<String>,
    inherited: Option<String>,
    _repo_root: Option<&str>,
) -> Option<String> {
    if let Some(setting) = queue_remote_launcher_setting(requested) {
        return setting.into_launcher();
    }
    current.or(inherited)
}

fn resolve_queue_remote_agent_proxy(requested: Option<&str>, env_name: &str) -> Option<String> {
    if let Some(setting) = queue_optional_setting(requested) {
        return setting.into_value();
    }
    env::var(env_name)
        .ok()
        .and_then(|value| queue_optional_setting(Some(&value)).and_then(QueueOptionalSetting::into_value))
}

fn resolve_queue_item_optional_setting(
    requested: Option<&str>,
    current: Option<String>,
    inherited: Option<String>,
) -> Option<String> {
    queue_optional_setting(requested)
        .map_or_else(|| current.or(inherited), QueueOptionalSetting::into_value)
}

fn queue_execution_host_setting(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    match value {
        "local" => Some("local".to_string()),
        "remote" | "remote-native" | "remote_native" => Some("remote-native".to_string()),
        _ => Some(value.to_string()),
    }
}

fn validate_queue_execution_hosts(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Result<()> {
    validate_queue_execution_host_value(&run.execution_host)?;
    for item in items {
        validate_queue_execution_host_value(&item.execution_host)?;
    }
    Ok(())
}

fn validate_queue_execution_host_value(value: &str) -> Result<()> {
    match value {
        "local" | "remote-native" => Ok(()),
        _ => bail!("unknown queue execution host: {value}"),
    }
}

enum QueueOptionalSetting {
    Clear,
    Value(String),
}

impl QueueOptionalSetting {
    fn into_value(self) -> Option<String> {
        match self {
            Self::Clear => None,
            Self::Value(value) => Some(value),
        }
    }
}

fn queue_optional_setting(value: Option<&str>) -> Option<QueueOptionalSetting> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if matches!(value, "local" | "none" | "off" | "false" | "0") {
        return Some(QueueOptionalSetting::Clear);
    }
    Some(QueueOptionalSetting::Value(value.to_string()))
}

enum QueueRemoteLauncherSetting {
    Local,
    Remote(String),
}

impl QueueRemoteLauncherSetting {
    fn into_launcher(self) -> Option<String> {
        match self {
            Self::Local => None,
            Self::Remote(value) => Some(value),
        }
    }
}

fn queue_remote_launcher_setting(value: Option<&str>) -> Option<QueueRemoteLauncherSetting> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if matches!(value, "local" | "none" | "off" | "false" | "0") {
        return Some(QueueRemoteLauncherSetting::Local);
    }
    Some(QueueRemoteLauncherSetting::Remote(value.to_string()))
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
    let mut references = HashMap::new();
    for item in items.iter() {
        references.insert(item.id.clone(), item.id.clone());
    }
    for item in items.iter() {
        references
            .entry(item.slug.clone())
            .or_insert_with(|| item.id.clone());
    }
    for item in items.iter_mut() {
        let item_id = item.id.clone();
        let mut seen = HashSet::new();
        item.depends_on = item
            .depends_on
            .iter()
            .filter_map(|dependency| references.get(dependency).cloned())
            .filter(|dependency_id| dependency_id != &item_id && seen.insert(dependency_id.clone()))
            .collect();
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
        state::request_web_queue_stop(Some(&run.id))?;
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
