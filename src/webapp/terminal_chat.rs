fn handle_terminal_send(
    headers: &HeaderMap,
    payload: &TerminalSendRequest,
) -> TerminalSendResponse {
    match handle_terminal_send_result(headers, payload) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "sent".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_terminal_send_result(headers: &HeaderMap, payload: &TerminalSendRequest) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let target = payload.target.trim();
    if target.is_empty() {
        bail!("terminal target is empty");
    }
    match terminal_input_from_request(payload)? {
        TerminalInput::Paste { text, submit } => send_terminal_paste(target, &text, submit),
        TerminalInput::Literal { text, submit } => send_terminal_literal(target, &text, submit),
        TerminalInput::Key(key) => send_terminal_key(target, key),
    }
}

fn handle_task_chat_target(
    headers: &HeaderMap,
    payload: &TaskChatTargetRequest,
) -> TaskChatResponse {
    match handle_task_chat_target_result(headers, payload) {
        Ok((target, agent_id)) => TaskChatResponse {
            ok: true,
            output: "target ready".to_string(),
            target,
            agent_id,
        },
        Err(err) => TaskChatResponse {
            ok: false,
            output: format!("{err:#}"),
            target: String::new(),
            agent_id: String::new(),
        },
    }
}

fn handle_task_chat_target_result(
    headers: &HeaderMap,
    payload: &TaskChatTargetRequest,
) -> Result<(String, String)> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    ensure_task_chat_target(&payload.task_id)
}

fn handle_task_chat_send(headers: &HeaderMap, payload: &TaskChatSendRequest) -> TaskChatResponse {
    match handle_task_chat_send_result(headers, payload) {
        Ok((target, agent_id)) => TaskChatResponse {
            ok: true,
            output: "sent".to_string(),
            target,
            agent_id,
        },
        Err(err) => TaskChatResponse {
            ok: false,
            output: format!("{err:#}"),
            target: String::new(),
            agent_id: String::new(),
        },
    }
}

fn handle_task_chat_send_result(
    headers: &HeaderMap,
    payload: &TaskChatSendRequest,
) -> Result<(String, String)> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let text = payload.text.trim_end();
    if text.trim().is_empty() {
        bail!("task chat message is empty");
    }
    let (target, agent_id) = if let Some(target) = payload
        .target
        .as_deref()
        .filter(|target| !target.trim().is_empty())
    {
        (clean_terminal_target(target)?, String::new())
    } else {
        ensure_task_chat_target(&payload.task_id)?
    };
    send_task_chat_text(&target, text)?;
    Ok((target, agent_id))
}

fn send_task_chat_text(target: &str, text: &str) -> Result<()> {
    if text.trim_start().starts_with('/') && !text.contains('\n') {
        send_terminal_literal(target, text, true)
    } else {
        send_terminal_paste(target, text, true)
    }
}

fn ensure_task_chat_target(task_id: &str) -> Result<(String, String)> {
    crate::sync_codex_task_records().ok();
    let mut record = task_record_by_id(task_id)?;
    let queue_item = queue_item_for_task_id(&record.id)?;
    if let Some(agent_id) = record
        .agent_id
        .clone()
        .or_else(|| queue_item.as_ref().and_then(|item| item.agent_id.clone()))
    {
        if let Some(target) = active_terminal_target_for_agent(&agent_id) {
            return Ok((target, agent_id));
        }
    }
    if record.status != "closed:blocked" && record.status != "paused" {
        bail!("task has no live chat target");
    }
    let session_id =
        codex_resume_session_id(&record).context("task has no Codex session id")?;
    let agent_command = queue_item
        .as_ref()
        .map(|item| item.agent_command.clone())
        .or_else(|| codex_command_from_metadata(record.metadata_json.as_deref()))
        .unwrap_or_else(|| "c1".to_string());
    if !agents::available_agent_commands()
        .iter()
        .any(|agent| agent.command == agent_command)
    {
        bail!("unknown task chat agent command: {agent_command}");
    }
    let command = format!("{agent_command} resume {}", shell_quote(&session_id));
    let cwd = record
        .cwd
        .as_deref()
        .filter(|path| Path::new(path).is_dir())
        .or(record.repo_root.as_deref().filter(|path| Path::new(path).is_dir()))
        .map(PathBuf::from);
    let request = AgentStartRequest {
        id: Some(task_chat_agent_id(&record.id)),
        cwd,
        track: "task-chat".to_string(),
        command,
    };
    let agent = start_web_agent(&request)?;
    let Some(target) = wait_for_agent_terminal_target(&agent.id) else {
        let _ = agents::terminate_agent(&agent.id);
        bail!("task chat terminal did not appear");
    };
    let _ = state::save_terminal_metadata(
        &target,
        Some(&task_chat_display_label(&record)),
        Some(&record.id),
    );
    thread::sleep(Duration::from_millis(500));
    if let Err(err) = send_task_chat_text(&target, &task_chat_resume_packet(&record)) {
        let _ = agents::terminate_agent(&agent.id);
        return Err(err);
    }
    record.agent_id = Some(agent.id.clone());
    state::upsert_task_record(&record)?;
    if let Some(item) = queue_item {
        state::set_web_queue_item_agent(&item.run_id, &item.id, &agent.id)?;
    }
    Ok((target, agent.id))
}

fn task_record_by_id(task_id: &str) -> Result<state::TaskRecordRow> {
    let task_id = task_id.trim();
    if task_id.is_empty() || task_id.chars().any(char::is_control) {
        bail!("invalid task id");
    }
    state::get_task_record(task_id)?.with_context(|| format!("unknown task record: {task_id}"))
}

fn active_terminal_target_for_agent(agent_id: &str) -> Option<String> {
    agents::terminal_contexts()
        .ok()?
        .into_iter()
        .find(|context| context.id == agent_id)
        .map(|context| context.target)
}

fn task_chat_agent_id(task_id: &str) -> String {
    let slug = task_id
        .strip_prefix("task/")
        .unwrap_or(task_id)
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        "task-chat".to_string()
    } else if slug.len() <= 36 {
        format!("chat-{slug}")
    } else {
        let prefix = slug.chars().take(24).collect::<String>();
        format!("chat-{prefix}-{}", stable_short_hash(task_id))
    }
}

fn task_chat_display_label(record: &state::TaskRecordRow) -> String {
    let slug = record.id.strip_prefix("task/").unwrap_or(&record.id);
    record
        .repo_root
        .as_deref()
        .and_then(|root| Path::new(root).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map_or_else(|| slug.to_string(), |name| format!("{name} {slug}"))
}

fn task_chat_resume_packet(record: &state::TaskRecordRow) -> String {
    let mut packet = String::new();
    let _ = writeln!(packet, "Q-COLD_RESUME_PACKET");
    let _ = writeln!(packet, "task_id: {}", record.id);
    if let Some(root) = record.repo_root.as_deref() {
        let _ = writeln!(packet, "repo_root: {root}");
    }
    if let Some(cwd) = record.cwd.as_deref().filter(|path| Path::new(path).is_dir()) {
        let _ = writeln!(packet, "cwd: {cwd}");
        for (label, path) in existing_task_state_paths(Path::new(cwd)) {
            let _ = writeln!(packet, "{label}: {}", path.display());
        }
    }
    if let Some(path) = codex_session_path_from_metadata(record.metadata_json.as_deref())
        .filter(|path| Path::new(path).is_file())
    {
        let _ = writeln!(packet, "codex_session_path: {path}");
    }
    let _ = writeln!(packet, "instruction: resume from visible task state only");
    let _ = writeln!(packet, "END_Q-COLD_RESUME_PACKET");
    packet
}

fn existing_task_state_paths(cwd: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut paths = Vec::new();
    let task_env = cwd.join(".task/task.env");
    if task_env.is_file() {
        paths.push(("task_env", task_env));
    }
    for (label, relative) in [
        ("task_logs", ".task/logs"),
        ("task_log", ".task/log"),
        ("task_iterations", ".task/iterations"),
    ] {
        let path = cwd.join(relative);
        if path.exists() {
            paths.push((label, path));
        }
    }
    paths
}

fn stable_short_hash(value: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let hex = format!("{hash:016x}");
    hex[8..].to_string()
}

fn handle_terminal_metadata(
    headers: &HeaderMap,
    payload: &TerminalMetadataRequest,
) -> TerminalSendResponse {
    match handle_terminal_metadata_result(headers, payload) {
        Ok(()) => TerminalSendResponse {
            ok: true,
            output: "saved".to_string(),
        },
        Err(err) => TerminalSendResponse {
            ok: false,
            output: format!("{err:#}"),
        },
    }
}

fn handle_terminal_metadata_result(
    headers: &HeaderMap,
    payload: &TerminalMetadataRequest,
) -> Result<()> {
    if webapp_write_token_required() {
        require_write_token(headers)?;
    }
    let target = clean_terminal_target(&payload.target)?;
    let name = clean_terminal_metadata_value(payload.name.as_deref());
    let scope = clean_terminal_metadata_value(payload.scope.as_deref());
    state::save_terminal_metadata(&target, name.as_deref(), scope.as_deref())
}

fn clean_terminal_target(target: &str) -> Result<String> {
    let target = target.trim();
    if target.is_empty() {
        bail!("terminal target is empty");
    }
    if target.len() > 200 || !target.contains(':') || target.chars().any(char::is_control) {
        bail!("invalid terminal target");
    }
    Ok(target.to_string())
}

fn clean_terminal_metadata_value(value: Option<&str>) -> Option<String> {
    let compact = value?.split_whitespace().collect::<Vec<_>>().join(" ");
    let compact = compact.trim();
    if compact.is_empty() {
        return None;
    }
    Some(truncate_chars(compact, 80))
}

fn task_transcript_response(task_id: &str) -> TaskTranscriptResponse {
    match task_transcript_result(task_id) {
        Ok(response) => response,
        Err(err) => TaskTranscriptResponse {
            ok: false,
            task_id: task_id.to_string(),
            title: String::new(),
            status: String::new(),
            session_path: None,
            chat_available: false,
            messages: Vec::new(),
            output: format!("{err:#}"),
        },
    }
}

fn task_transcript_result(task_id: &str) -> Result<TaskTranscriptResponse> {
    let task_id = task_id.trim();
    if task_id.is_empty() || task_id.chars().any(char::is_control) {
        bail!("invalid task id");
    }
    crate::sync_codex_task_records().ok();
    let record = state::get_task_record(task_id)?
        .with_context(|| format!("unknown task record: {task_id}"))?;
    let session_path = codex_session_path_from_metadata(record.metadata_json.as_deref())
        .context("task record has no Codex session transcript")?;
    let path = PathBuf::from(&session_path);
    if !is_codex_session_path(&path) {
        bail!("refusing to read non-Codex session path: {}", path.display());
    }
    let messages = codex_transcript_messages(&path)?;
    let chat_available = task_record_chat_available(&record);
    Ok(TaskTranscriptResponse {
        ok: true,
        task_id: record.id,
        title: record.title,
        chat_available,
        status: record.status,
        session_path: Some(session_path),
        messages,
        output: String::new(),
    })
}

fn task_record_chat_available(record: &state::TaskRecordRow) -> bool {
    matches!(record.status.as_str(), "closed:blocked" | "paused")
        && codex_resume_session_id(record).is_some()
}

fn codex_session_path_from_metadata(metadata_json: Option<&str>) -> Option<String> {
    let metadata = serde_json::from_str::<Value>(metadata_json?).ok()?;
    metadata
        .get("session_path")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn codex_command_from_metadata(metadata_json: Option<&str>) -> Option<String> {
    let metadata = serde_json::from_str::<Value>(metadata_json?).ok()?;
    metadata
        .get("command")
        .and_then(Value::as_str)
        .and_then(|command| shell_words(command).into_iter().next())
}

fn codex_resume_session_id(record: &state::TaskRecordRow) -> Option<String> {
    let metadata = serde_json::from_str::<Value>(record.metadata_json.as_deref()?).ok()?;
    metadata
        .get("codex_thread_id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            metadata
                .get("session_path")
                .and_then(Value::as_str)
                .and_then(codex_thread_id_from_session_path)
        })
}

fn codex_thread_id_from_session_path(path: &str) -> Option<String> {
    let stem = Path::new(path).file_stem()?.to_str()?;
    let id = stem.get(stem.len().saturating_sub(36)..)?;
    if id.len() == 36
        && id.chars().enumerate().all(|(index, ch)| {
            matches!(index, 8 | 13 | 18 | 23) && ch == '-'
                || !matches!(index, 8 | 13 | 18 | 23) && ch.is_ascii_hexdigit()
        })
    {
        Some(id.to_string())
    } else {
        None
    }
}

fn queue_item_for_task_id(task_id: &str) -> Result<Option<state::QueueItemRow>> {
    let (_, items) = state::load_web_queue()?;
    let Some(slug) = task_id.strip_prefix("task/") else {
        return Ok(None);
    };
    Ok(items.into_iter().find(|item| item.slug == slug))
}

fn is_codex_session_path(path: &Path) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let Ok(home) = home.canonicalize() else {
        return false;
    };
    let account_root = home.join(".codex-accounts");
    let default_root = home.join(".codex").join("sessions");
    (path.starts_with(account_root) && path.components().any(|part| part.as_os_str() == "sessions"))
        || path.starts_with(default_root)
}

fn codex_transcript_messages(path: &Path) -> Result<Vec<TaskTranscriptMessage>> {
    let file = File::open(path)
        .with_context(|| format!("failed to open Codex session {}", path.display()))?;
    let mut messages = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.with_context(|| format!("failed to read Codex session {}", path.display()))?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(message) = transcript_message_from_json(&value) {
            push_transcript_message(&mut messages, message);
        }
    }
    Ok(messages)
}

fn transcript_message_from_json(value: &Value) -> Option<TaskTranscriptMessage> {
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match value.get("type").and_then(Value::as_str)? {
        "event_msg" => {
            let payload = value.get("payload")?;
            match payload.get("type").and_then(Value::as_str)? {
                "user_message" => transcript_message(
                    timestamp,
                    "user",
                    payload.get("message").and_then(Value::as_str)?,
                ),
                "agent_message" => transcript_message(
                    timestamp,
                    "assistant",
                    payload.get("message").and_then(Value::as_str)?,
                ),
                _ => None,
            }
        }
        "response_item" => {
            let payload = value.get("payload")?;
            if payload.get("type").and_then(Value::as_str) != Some("message") {
                return None;
            }
            let role = payload.get("role").and_then(Value::as_str)?;
            if !matches!(role, "user" | "assistant") {
                return None;
            }
            transcript_message(timestamp, role, &response_content_text(payload)?)
        }
        _ => None,
    }
}

fn transcript_message(timestamp: String, role: &str, text: &str) -> Option<TaskTranscriptMessage> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(TaskTranscriptMessage {
        timestamp,
        role: role.to_string(),
        text: truncate_chars(text, 30_000),
    })
}

fn response_content_text(payload: &Value) -> Option<String> {
    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|item| {
            let item_type = item.get("type").and_then(Value::as_str)?;
            if !matches!(item_type, "input_text" | "output_text" | "text") {
                return None;
            }
            item.get("text").and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn push_transcript_message(messages: &mut Vec<TaskTranscriptMessage>, message: TaskTranscriptMessage) {
    if messages
        .last()
        .is_some_and(|last| last.role == message.role && last.text == message.text)
    {
        return;
    }
    messages.push(message);
}

fn parse_zellij_target(target: &str) -> Option<(&str, &str)> {
    let rest = target.strip_prefix("zellij:")?;
    rest.split_once(':')
}

enum TerminalInput {
    Paste { text: String, submit: bool },
    Literal { text: String, submit: bool },
    Key(TerminalKey),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum TerminalKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Backspace,
    Delete,
    Escape,
    Tab,
    Home,
    End,
    PageUp,
    PageDown,
}

impl TerminalKey {
    fn tmux(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Enter => "Enter",
            Self::Backspace => "BSpace",
            Self::Delete => "DC",
            Self::Escape => "Escape",
            Self::Tab => "Tab",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
        }
    }

    fn zellij(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Enter => "Enter",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Escape => "Esc",
            Self::Tab => "Tab",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
        }
    }
}

fn terminal_input_from_request(payload: &TerminalSendRequest) -> Result<TerminalInput> {
    let mode = payload.mode.as_deref().unwrap_or("paste").trim();
    match mode {
        "key" => {
            let key = payload
                .key
                .as_deref()
                .context("terminal key is empty")?;
            Ok(TerminalInput::Key(clean_terminal_key(key)?))
        }
        "literal" => {
            let text = terminal_request_text(payload)?;
            Ok(TerminalInput::Literal {
                text,
                submit: payload.submit.unwrap_or(false),
            })
        }
        "paste" | "" => {
            let text = terminal_request_text(payload)?;
            Ok(TerminalInput::Paste {
                text,
                submit: payload.submit.unwrap_or(true),
            })
        }
        _ => bail!("unsupported terminal input mode"),
    }
}

fn terminal_request_text(payload: &TerminalSendRequest) -> Result<String> {
    let text = payload.text.as_deref().unwrap_or_default().trim_end();
    if text.trim().is_empty() {
        bail!("terminal input is empty");
    }
    Ok(text.to_string())
}

fn clean_terminal_key(key: &str) -> Result<TerminalKey> {
    match key.trim() {
        "Up" | "ArrowUp" => Ok(TerminalKey::Up),
        "Down" | "ArrowDown" => Ok(TerminalKey::Down),
        "Left" | "ArrowLeft" => Ok(TerminalKey::Left),
        "Right" | "ArrowRight" => Ok(TerminalKey::Right),
        "Enter" => Ok(TerminalKey::Enter),
        "Backspace" => Ok(TerminalKey::Backspace),
        "Delete" => Ok(TerminalKey::Delete),
        "Escape" | "Esc" => Ok(TerminalKey::Escape),
        "Tab" => Ok(TerminalKey::Tab),
        "Home" => Ok(TerminalKey::Home),
        "End" => Ok(TerminalKey::End),
        "PageUp" => Ok(TerminalKey::PageUp),
        "PageDown" => Ok(TerminalKey::PageDown),
        _ => bail!("unsupported terminal key"),
    }
}

fn send_terminal_paste(target: &str, text: &str, submit: bool) -> Result<()> {
    if let Some((session, pane)) = parse_zellij_target(target) {
        send_zellij_terminal_paste(session, pane, text, submit)
    } else {
        send_tmux_terminal_paste(target, text, submit)
    }
}

fn send_terminal_literal(target: &str, text: &str, submit: bool) -> Result<()> {
    if let Some((session, pane)) = parse_zellij_target(target) {
        send_zellij_terminal_literal(session, pane, text, submit)
    } else {
        send_tmux_terminal_literal(target, text, submit)
    }
}

fn send_terminal_key(target: &str, key: TerminalKey) -> Result<()> {
    if let Some((session, pane)) = parse_zellij_target(target) {
        send_zellij_terminal_key(session, pane, key)
    } else {
        send_tmux_terminal_key(target, key)
    }
}

fn send_tmux_terminal_paste(target: &str, text: &str, submit: bool) -> Result<()> {
    paste_terminal_text(target, text)?;
    if submit {
        thread::sleep(terminal_paste_submit_delay(text));
        send_tmux_terminal_key(target, TerminalKey::Enter)?;
    }
    Ok(())
}

fn send_tmux_terminal_literal(target: &str, text: &str, submit: bool) -> Result<()> {
    let status = Command::new("tmux")
        .args(["send-keys", "-t", target, "-l", text])
        .status()
        .context("failed to send literal terminal input through tmux")?;
    if !status.success() {
        bail!("tmux send-keys literal failed with {status}");
    }
    if submit {
        send_tmux_terminal_key(target, TerminalKey::Enter)?;
    }
    Ok(())
}

fn send_tmux_terminal_key(target: &str, key: TerminalKey) -> Result<()> {
    let status = Command::new("tmux")
        .args(["send-keys", "-t", target, key.tmux()])
        .status()
        .context("failed to send terminal key through tmux")?;
    if !status.success() {
        bail!("tmux send-keys failed with {status}");
    }
    Ok(())
}

fn send_zellij_terminal_paste(session: &str, pane: &str, text: &str, submit: bool) -> Result<()> {
    let status = Command::new("zellij")
        .args(["--session", session, "action", "paste", "--pane-id", pane, text])
        .status()
        .context("failed to paste terminal input through zellij")?;
    if !status.success() {
        bail!("zellij action paste failed with {status}");
    }
    if submit {
        thread::sleep(terminal_paste_submit_delay(text));
        send_zellij_terminal_key(session, pane, TerminalKey::Enter)?;
    }
    Ok(())
}

fn send_zellij_terminal_literal(session: &str, pane: &str, text: &str, submit: bool) -> Result<()> {
    let status = Command::new("zellij")
        .args([
            "--session",
            session,
            "action",
            "write-chars",
            "--pane-id",
            pane,
            text,
        ])
        .status()
        .context("failed to write terminal input through zellij")?;
    if !status.success() {
        bail!("zellij action write-chars failed with {status}");
    }
    if submit {
        send_zellij_terminal_key(session, pane, TerminalKey::Enter)?;
    }
    Ok(())
}

fn send_zellij_terminal_key(session: &str, pane: &str, key: TerminalKey) -> Result<()> {
    let status = Command::new("zellij")
        .args([
            "--session",
            session,
            "action",
            "send-keys",
            "--pane-id",
            pane,
            key.zellij(),
        ])
        .status()
        .context("failed to send terminal key through zellij")?;
    if !status.success() {
        bail!("zellij action send-keys failed with {status}");
    }
    Ok(())
}

fn paste_terminal_text(target: &str, text: &str) -> Result<()> {
    let buffer = terminal_paste_buffer_name()?;
    let mut child = Command::new("tmux")
        .args(["load-buffer", "-b", &buffer, "-"])
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to load terminal input into tmux buffer")?;
    let mut stdin = child
        .stdin
        .take()
        .context("failed to open tmux buffer stdin")?;
    stdin
        .write_all(text.as_bytes())
        .context("failed to write terminal input to tmux buffer")?;
    drop(stdin);
    let status = child
        .wait()
        .context("failed waiting for tmux load-buffer")?;
    if !status.success() {
        bail!("tmux load-buffer failed with {status}");
    }

    let status = Command::new("tmux")
        .args(["paste-buffer", "-d", "-b", &buffer, "-t", target])
        .status()
        .context("failed to paste terminal input through tmux")?;
    if !status.success() {
        bail!("tmux paste-buffer failed with {status}");
    }
    Ok(())
}

fn terminal_paste_buffer_name() -> Result<String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_nanos();
    Ok(format!("qcold-web-send-{}-{nanos}", std::process::id()))
}

fn terminal_paste_submit_delay(text: &str) -> Duration {
    if text.contains('\n') || text.len() > 1024 {
        Duration::from_millis(1500)
    } else {
        Duration::from_millis(100)
    }
}

fn require_write_token(headers: &HeaderMap) -> Result<()> {
    let expected = optional_env("QCOLD_WEBAPP_WRITE_TOKEN")
        .context("set QCOLD_WEBAPP_WRITE_TOKEN before enabling GUI command execution")?;
    let provided = headers
        .get("x-qcold-write-token")
        .and_then(|value| value.to_str().ok())
        .context("missing X-QCOLD-Write-Token header")?;
    if provided != expected {
        bail!("invalid GUI write token");
    }
    Ok(())
}

fn webapp_write_token_required() -> bool {
    optional_env("QCOLD_WEBAPP_REQUIRE_WRITE_TOKEN")
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

fn start_web_agent(request: &AgentStartRequest) -> Result<agents::AgentRecord> {
    if let Some(cwd) = request.cwd.clone() {
        agents::start_terminal_shell_agent_with_id_in_cwd(
            request.id.clone(),
            &request.track,
            &request.command,
            &cwd,
        )
    } else {
        agents::start_terminal_shell_agent_with_id(
            request.id.clone(),
            &request.track,
            &request.command,
        )
    }
}

fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escape = false;
    for ch in command.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        match quote {
            Some(q) if ch == q => quote = None,
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            Some(_) | None => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn sanitize_daemon_id(value: &str) -> String {
    let id = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let id = id.trim_matches('-');
    if id.is_empty() {
        "default".to_string()
    } else {
        id.to_string()
    }
}
