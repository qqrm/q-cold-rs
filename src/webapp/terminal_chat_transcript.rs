const REMOTE_NATIVE_TRANSCRIPT_READ_TIMEOUT: &str = "20s";

fn task_transcript_response(task_id: &str) -> TaskTranscriptResponse {
    match task_transcript_result(task_id) {
        Ok(response) => response,
        Err(err) => TaskTranscriptResponse {
            ok: false,
            task_id: task_id.to_string(),
            title: String::new(),
            status: String::new(),
            session_path: None,
            transcript_path: None,
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
    let queue_item = queue_item_for_task_id(task_id)?;
    let Some(record) = task_record_for_transcript(task_id)? else {
        if let Some(item) = queue_item {
            request_queue_item_reconcile_from_task_chat(&item);
            return Ok(queue_item_transcript_response(
                task_id,
                &item,
                queue_item_without_task_record_message(&item),
            ));
        }
        bail!("unknown task record: {task_id}");
    };
    if live_web_queue_task_without_closed_status(&record) {
        if let Some(response) = task_agent_execution_log_response(&record)? {
            return Ok(response);
        }
        if let Some(item) = queue_item.as_ref().filter(|item| queue_item_remote_native(item)) {
            if let Some(response) = task_session_transcript_response(&record, Some(item))? {
                return Ok(response);
            }
        }
        if let Some(item) = queue_item {
            request_queue_item_reconcile_from_task_chat(&item);
            return Ok(queue_item_transcript_response(
                task_id,
                &item,
                queue_item_without_transcript_message(&item),
            ));
        }
        bail!("live queue tasks are available through their executor terminal only");
    }
    if let Some(response) = task_session_transcript_response(&record, queue_item.as_ref())? {
        return Ok(response);
    }
    if let Some(response) = task_agent_execution_log_response(&record)? {
        return Ok(response);
    }
    if let Some(item) = queue_item {
        request_queue_item_reconcile_from_task_chat(&item);
        return Ok(queue_item_transcript_response(
            task_id,
            &item,
            queue_item_without_transcript_message(&item),
        ));
    }
    bail!("task record has no Codex session transcript or task execution log")
}

fn task_session_transcript_response(
    record: &state::TaskRecordRow,
    queue_item: Option<&state::QueueItemRow>,
) -> Result<Option<TaskTranscriptResponse>> {
    let Some(session_path) = codex_session_path_from_metadata(record.metadata_json.as_deref()) else {
        return Ok(None);
    };
    let path = PathBuf::from(&session_path);
    let messages = if is_codex_session_path(&path) {
        codex_transcript_messages(&path)?
    } else if let Some(item) = queue_item {
        remote_native_codex_transcript_messages(item, &session_path)?.with_context(|| {
            format!("refusing to read non-Codex session path: {}", path.display())
        })?
    } else {
        bail!("refusing to read non-Codex session path: {}", path.display());
    };
    let chat_available = task_record_chat_available(record)
        || queue_item
            .and_then(active_queue_item_terminal_target)
            .is_some();
    Ok(Some(TaskTranscriptResponse {
        ok: true,
        task_id: record.id.clone(),
        title: record.title.clone(),
        chat_available,
        status: record.status.clone(),
        session_path: Some(session_path),
        transcript_path: None,
        messages,
        output: String::new(),
    }))
}

fn task_record_for_transcript(task_id: &str) -> Result<Option<state::TaskRecordRow>> {
    if let Some(record) = state::get_task_record(task_id)? {
        return Ok(Some(record));
    }
    crate::sync_codex_task_records().ok();
    state::get_task_record(task_id)
}

fn queue_item_transcript_response(
    task_id: &str,
    item: &state::QueueItemRow,
    message: String,
) -> TaskTranscriptResponse {
    TaskTranscriptResponse {
        ok: true,
        task_id: task_id.to_string(),
        title: item.slug.clone(),
        chat_available: active_queue_item_terminal_target(item).is_some(),
        status: item.status.as_str().to_string(),
        session_path: None,
        transcript_path: None,
        messages: transcript_message(String::new(), "system", &message)
            .into_iter()
            .collect(),
        output: message,
    }
}

fn task_agent_execution_log_response(
    record: &state::TaskRecordRow,
) -> Result<Option<TaskTranscriptResponse>> {
    let Some(path) = task_agent_execution_log_path(record) else {
        return Ok(None);
    };
    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read task execution log {}", path.display()))?;
    let messages = transcript_message(String::new(), "assistant", &text).into_iter().collect();
    Ok(Some(TaskTranscriptResponse {
        ok: true,
        task_id: record.id.clone(),
        title: record.title.clone(),
        chat_available: task_record_chat_available(record),
        status: record.status.clone(),
        session_path: None,
        transcript_path: Some(path.display().to_string()),
        messages,
        output: String::new(),
    }))
}

fn task_agent_execution_log_path(record: &state::TaskRecordRow) -> Option<PathBuf> {
    let worktree = task_metadata_string(record.metadata_json.as_deref(), "task_worktree")
        .or_else(|| record.cwd.clone())
        .map(PathBuf::from)?;
    if !worktree.is_absolute() {
        return None;
    }
    let logs_dir = worktree.join(".task/logs");
    let path = logs_dir.join("agent-execution.md");
    if !path.is_file() || !task_log_path_is_inside_worktree(&worktree, &logs_dir, &path) {
        return None;
    }
    Some(path)
}

fn task_log_path_is_inside_worktree(worktree: &Path, logs_dir: &Path, path: &Path) -> bool {
    let (Ok(worktree), Ok(logs_dir), Ok(path)) = (
        worktree.canonicalize(),
        logs_dir.canonicalize(),
        path.canonicalize(),
    ) else {
        return false;
    };
    logs_dir.starts_with(&worktree) && path.starts_with(&logs_dir)
}

fn task_record_chat_available(record: &state::TaskRecordRow) -> bool {
    matches!(record.status.as_str(), "closed:blocked" | "paused")
        && codex_resume_session_id(record).is_some()
}

fn live_web_queue_task_without_closed_status(record: &state::TaskRecordRow) -> bool {
    !record.status.starts_with("closed:")
        && task_metadata_string(record.metadata_json.as_deref(), "opened_by")
            .is_some_and(|value| value == "web-queue")
}

fn codex_session_path_from_metadata(metadata_json: Option<&str>) -> Option<String> {
    task_metadata_string(metadata_json, "session_path")
}

fn remote_native_codex_transcript_messages(
    item: &state::QueueItemRow,
    session_path: &str,
) -> Result<Option<Vec<TaskTranscriptMessage>>> {
    if !queue_item_remote_native(item) {
        return Ok(None);
    }
    if !remote_codex_session_path_allowed(session_path) {
        return Ok(None);
    }
    let Some(launcher) = item
        .remote_launcher
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let output = Command::new("timeout")
        .arg(REMOTE_NATIVE_TRANSCRIPT_READ_TIMEOUT)
        .arg(launcher)
        .args(["cat", "--", session_path])
        .output()
        .with_context(|| format!("failed to read remote Codex session {session_path}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "remote Codex session read failed with {}: {}",
            output.status,
            stderr.trim()
        );
    }
    let reader = BufReader::new(output.stdout.as_slice());
    codex_transcript_messages_from_reader(reader, session_path).map(Some)
}

fn remote_codex_session_path_allowed(path: &str) -> bool {
    if path.trim().is_empty() || path.chars().any(char::is_control) {
        return false;
    }
    let path = Path::new(path);
    path.is_absolute()
        && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
        && path
            .components()
            .any(|part| part.as_os_str().to_string_lossy() == "sessions")
}

fn task_metadata_string(metadata_json: Option<&str>, key: &str) -> Option<String> {
    let metadata = serde_json::from_str::<Value>(metadata_json?).ok()?;
    metadata
        .get(key)
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
