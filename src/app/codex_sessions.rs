fn find_codex_session_summary(
    account: &str,
    agent_started_at: u64,
    claimed_session_paths: &HashSet<String>,
    preferred_cwd: Option<&Path>,
) -> Result<Option<CodexSessionSummary>> {
    let root = codex_sessions_root(account)?;
    find_codex_session_summary_in_root(
        &root,
        agent_started_at,
        claimed_session_paths,
        preferred_cwd,
    )
}

fn codex_task_telemetry_for_worktree(
    worktree: &Path,
    task_slug: Option<&str>,
    started_at: u64,
    finished_at: u64,
    explicit_rollout_paths: &[PathBuf],
    explicit_thread_id: Option<&str>,
) -> Result<Option<CodexTaskTelemetry>> {
    codex_task_telemetry_for_worktree_in_roots(
        worktree,
        task_slug,
        started_at,
        finished_at,
        &codex_session_roots()?,
        Some(codex_telemetry_retention_cutoff(unix_now())),
        explicit_rollout_paths,
        explicit_thread_id,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "testable telemetry helper keeps scan inputs explicit"
)]
fn codex_task_telemetry_for_worktree_in_roots(
    worktree: &Path,
    task_slug: Option<&str>,
    started_at: u64,
    finished_at: u64,
    roots: &[PathBuf],
    retention_cutoff: Option<u64>,
    explicit_rollout_paths: &[PathBuf],
    explicit_thread_id: Option<&str>,
) -> Result<Option<CodexTaskTelemetry>> {
    let mut files = Vec::new();
    for root in roots {
        if root.is_dir() {
            collect_session_files(root, &mut files)?;
        }
    }
    let worktree_text = worktree.display().to_string();
    let task_terms = task_slug
        .into_iter()
        .flat_map(|slug| [slug.to_string(), format!("task/{slug}")])
        .collect::<Vec<_>>();
    let mut usage = CodexTokenUsage::default();
    let mut telemetry = CodexTaskTelemetry {
        retention_hours: codex_telemetry_retention_hours(),
        ..CodexTaskTelemetry::default()
    };
    let scan_started_at = retention_cutoff
        .unwrap_or(0)
        .max(started_at.saturating_sub(300));
    let mut explicit_path_texts = HashSet::new();
    let mut explicit_paths = explicit_rollout_paths.to_vec();
    if let Some(thread_id) = explicit_thread_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let existing = explicit_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<HashSet<_>>();
        explicit_paths.extend(files.iter().filter_map(|path| {
            let path_text = path.display().to_string();
            let name_matches = path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.contains(thread_id));
            (name_matches && !existing.contains(&path_text)).then(|| path.clone())
        }));
    }
    for path in explicit_paths.iter().filter(|path| path.is_file()) {
        let content = fs::read_to_string(path)?;
        let Some(session_id) = codex_session_match_for_explicit_rollout(
            path,
            &content,
            explicit_thread_id,
        ) else {
            continue;
        };
        explicit_path_texts.insert(path.display().to_string());
        collect_codex_task_telemetry_session(
            path,
            &content,
            session_id,
            CodexTaskSessionMatch::Explicit,
            &task_terms,
            started_at,
            finished_at,
            &mut usage,
            &mut telemetry,
        );
    }
    for path in files {
        if explicit_path_texts.contains(&path.display().to_string()) {
            continue;
        }
        let Some(modified) = modified_unix(&path) else {
            continue;
        };
        if modified < scan_started_at || modified > finished_at.saturating_add(900) {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        let Some(session_id) = codex_session_match_for_worktree(&content, &worktree_text) else {
            continue;
        };
        collect_codex_task_telemetry_session(
            &path,
            &content,
            session_id,
            CodexTaskSessionMatch::Worktree,
            &task_terms,
            started_at,
            finished_at,
            &mut usage,
            &mut telemetry,
        );
    }
    telemetry.usage = usage;
    Ok((telemetry.usage.model_calls > 0 || telemetry.tool_outputs.calls > 0).then_some(telemetry))
}

#[derive(Clone, Copy)]
enum CodexTaskSessionMatch {
    Explicit,
    Worktree,
}

#[allow(clippy::too_many_arguments, reason = "session telemetry aggregation carries a compact state tuple")]
fn collect_codex_task_telemetry_session(
    path: &Path,
    content: &str,
    session_id: String,
    match_kind: CodexTaskSessionMatch,
    task_terms: &[String],
    started_at: u64,
    finished_at: u64,
    usage: &mut CodexTokenUsage,
    telemetry: &mut CodexTaskTelemetry,
) {
    let task_match = task_terms.iter().any(|term| content.contains(term));
    telemetry.session_count += 1;
    let session_path = path.display().to_string();
    if !telemetry.session_ids.contains(&session_id) {
        telemetry.session_ids.push(session_id);
    }
    if !telemetry.session_paths.contains(&session_path) {
        telemetry.session_paths.push(session_path);
    }
    match match_kind {
        CodexTaskSessionMatch::Explicit => telemetry.matched_by_explicit += 1,
        CodexTaskSessionMatch::Worktree => telemetry.matched_by_worktree += 1,
    }
    if task_match {
        telemetry.matched_by_task += 1;
    }
    let session_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("session.jsonl")
        .to_string();
    let mut calls = HashMap::new();
    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_unix);
        let within_window =
            timestamp.is_some_and(|timestamp| timestamp >= started_at && timestamp <= finished_at);
        if value.get("type").and_then(Value::as_str) == Some("response_item") {
            if let Some(payload) = value.get("payload") {
                collect_tool_output_stats(
                    payload,
                    &session_name,
                    &mut calls,
                    telemetry,
                    within_window,
                );
            }
        }
        if !within_window {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("event_msg")
            && payload.get("type").and_then(Value::as_str) == Some("token_count")
        {
            if let Some(info) = payload.get("info") {
                usage.add_last_usage(info);
            }
        }
    }
}

fn codex_session_match_for_explicit_rollout(
    path: &Path,
    content: &str,
    explicit_thread_id: Option<&str>,
) -> Option<String> {
    let session_id = codex_session_id_from_content(content)
        .or_else(|| codex_thread_id_from_path(path))?;
    if explicit_thread_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|thread_id| thread_id != session_id)
    {
        return None;
    }
    Some(session_id)
}

fn codex_session_id_from_content(content: &str) -> Option<String> {
    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            return value
                .get("payload")
                .and_then(|payload| payload.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
    }
    None
}

fn codex_session_match_for_worktree(content: &str, worktree_text: &str) -> Option<String> {
    let mut session_id = None;
    let mut matches_worktree = false;
    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            let payload = value.get("payload");
            if session_id.is_none() {
                session_id = payload
                    .and_then(|payload| payload.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            matches_worktree |= payload
                .and_then(|payload| payload.get("cwd"))
                .and_then(Value::as_str)
                .is_some_and(|cwd| path_text_is_in_worktree(cwd, worktree_text));
        }
        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        match payload.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let Some(arguments) = payload.get("arguments").and_then(Value::as_str) else {
                    continue;
                };
                let Ok(arguments) = serde_json::from_str::<Value>(arguments) else {
                    continue;
                };
                matches_worktree |= function_call_matches_worktree(&arguments, worktree_text);
            }
            Some("function_call_output") => {
                matches_worktree |= payload
                    .get("output")
                    .and_then(Value::as_str)
                    .is_some_and(|output| output_mentions_worktree_marker(output, worktree_text));
            }
            _ => {}
        }
    }
    matches_worktree
        .then_some(session_id?)
        .filter(|id| !id.is_empty())
}

fn function_call_matches_worktree(arguments: &Value, worktree_text: &str) -> bool {
    ["workdir", "cwd"].iter().any(|key| {
        arguments
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|path| path_text_is_in_worktree(path, worktree_text))
    }) || value_mentions_managed_path(arguments, worktree_text)
}

fn value_mentions_managed_path(value: &Value, worktree_text: &str) -> bool {
    match value {
        Value::String(text) => text.contains(worktree_text)
            || output_mentions_worktree_marker(text, worktree_text),
        Value::Array(items) => items
            .iter()
            .any(|item| value_mentions_managed_path(item, worktree_text)),
        Value::Object(items) => items
            .values()
            .any(|item| value_mentions_managed_path(item, worktree_text)),
        _ => false,
    }
}

fn output_mentions_worktree_marker(output: &str, worktree_text: &str) -> bool {
    ["TASK_WORKTREE", "QCOLD_REPO_ROOT", "VITASTOR_TASKFLOW_TASK_WORKTREE"]
        .iter()
        .any(|key| {
            [
                format!("{key}={worktree_text}"),
                format!("{key}='{worktree_text}'"),
                format!("{key}=\"{worktree_text}\""),
            ]
            .iter()
            .any(|needle| output.contains(needle))
        })
}

fn path_text_is_in_worktree(path: &str, worktree_text: &str) -> bool {
    path == worktree_text
        || path
            .strip_prefix(worktree_text)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn codex_telemetry_retention_hours() -> u64 {
    std::env::var("QCOLD_CODEX_TELEMETRY_RETENTION_HOURS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_CODEX_TELEMETRY_RETENTION_HOURS)
}

fn codex_telemetry_retention_cutoff(now: u64) -> u64 {
    now.saturating_sub(codex_telemetry_retention_hours().saturating_mul(60 * 60))
}

fn collect_tool_output_stats(
    payload: &Value,
    session_name: &str,
    calls: &mut HashMap<String, String>,
    telemetry: &mut CodexTaskTelemetry,
    count_output: bool,
) {
    match payload.get("type").and_then(Value::as_str) {
        Some("function_call") => {
            if let Some(call_id) = payload.get("call_id").and_then(Value::as_str) {
                calls.insert(call_id.to_string(), compact_function_call(payload));
            }
        }
        Some("function_call_output") => {
            if !count_output {
                return;
            }
            let Some(output) = payload.get("output").and_then(Value::as_str) else {
                return;
            };
            let Some(tokens) = original_token_count(output) else {
                return;
            };
            let command = payload
                .get("call_id")
                .and_then(Value::as_str)
                .and_then(|call_id| calls.get(call_id))
                .cloned()
                .unwrap_or_else(|| "unknown tool call".to_string());
            telemetry.tool_outputs.add_sample(CodexToolOutputSample {
                original_tokens: tokens,
                session: session_name.to_string(),
                command,
            });
        }
        _ => {}
    }
}

fn compact_function_call(payload: &Value) -> String {
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("function_call");
    let arguments = payload
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .replace(['\n', '\r', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    compact_text(&format!("{name} {arguments}"), 180)
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let mut text = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().count() <= max_chars {
        return text;
    }
    text = text.chars().take(max_chars.saturating_sub(3)).collect();
    text.push_str("...");
    text
}

fn original_token_count(output: &str) -> Option<u64> {
    output
        .split("Original token count:")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
}

fn codex_session_roots() -> Result<Vec<PathBuf>> {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        return Ok(vec![PathBuf::from(home).join("sessions")]);
    }
    let home = std::env::var("HOME").context("HOME is required to locate Codex session telemetry")?;
    let accounts = PathBuf::from(home).join(".codex-accounts");
    let mut roots = Vec::new();
    if accounts.is_dir() {
        for entry in fs::read_dir(accounts)? {
            let path = entry?.path().join("sessions");
            if path.is_dir() {
                roots.push(path);
            }
        }
    }
    Ok(roots)
}

fn find_codex_session_summary_in_root(
    root: &Path,
    agent_started_at: u64,
    claimed_session_paths: &HashSet<String>,
    preferred_cwd: Option<&Path>,
) -> Result<Option<CodexSessionSummary>> {
    if !root.exists() {
        return Ok(None);
    }

    let mut files = Vec::new();
    collect_session_files(root, &mut files)?;
    let cutoff = agent_started_at.saturating_sub(300);
    let mut files = files
        .into_iter()
        .filter_map(|path| {
            let modified = modified_unix(&path)?;
            Some((path, modified))
        })
        .filter(|(path, modified)| {
            let path_display = path.display().to_string();
            if claimed_session_paths.contains(&path_display) {
                return false;
            }
            *modified >= cutoff
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    files.truncate(100);

    let mut candidates = Vec::new();
    for (path, modified) in files {
        if let Some(summary) = parse_codex_session_summary(&path)? {
            if !codex_session_start_matches_agent(summary.started_at, agent_started_at) {
                continue;
            }
            let cwd_mismatch = !codex_session_cwd_matches(summary.cwd.as_deref(), preferred_cwd);
            let start_distance = summary
                .started_at
                .map_or(u64::MAX, |started_at| started_at.abs_diff(agent_started_at));
            candidates.push((cwd_mismatch, start_distance, Reverse(modified), summary));
        }
    }
    candidates.sort_by_key(|(cwd_mismatch, start_distance, modified, _)| {
        (*cwd_mismatch, *start_distance, *modified)
    });
    Ok(candidates.into_iter().next().map(|(_, _, _, summary)| summary))
}

fn codex_sessions_root(account: &str) -> Result<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home)
            .join(".codex-accounts")
            .join(account)
            .join("sessions"));
    }
    anyhow::bail!("HOME is required to locate Codex session telemetry")
}

fn collect_session_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_session_files(&path, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    Ok(())
}

fn parse_codex_session_summary(path: &Path) -> Result<Option<CodexSessionSummary>> {
    let content = fs::read_to_string(path)?;
    let mut thread_id = None;
    let mut started_at = None;
    let mut cwd = None;
    let mut prompt = None;
    let mut fallback_prompt = None;
    let mut token_usage = None;
    let mut rate_limits = None;
    let mut task_complete = false;

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                if thread_id.is_none() {
                    thread_id = payload
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                if started_at.is_none() {
                    started_at = payload
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .and_then(parse_rfc3339_unix);
                }
                if cwd.is_none() {
                    cwd = payload
                        .get("cwd")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
            }
            Some("event_msg") => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                match payload.get("type").and_then(Value::as_str) {
                    Some("user_message") if prompt.is_none() => {
                        if let Some(message) = payload.get("message").and_then(Value::as_str) {
                            if is_meaningful_task_prompt(message) {
                                prompt = Some(message.trim().to_string());
                            }
                        }
                    }
                    Some("token_count") => {
                        if let Some(usage) = token_usage_summary(payload) {
                            token_usage = Some(usage);
                            rate_limits = payload.get("rate_limits").cloned();
                        }
                    }
                    Some("task_complete") => task_complete = true,
                    _ => {}
                }
            }
            Some("response_item") if fallback_prompt.is_none() => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                if payload.get("role").and_then(Value::as_str) == Some("user") {
                    if let Some(message) = response_item_text(payload) {
                        if is_meaningful_task_prompt(&message) {
                            fallback_prompt = Some(message);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let prompt = prompt.or(fallback_prompt);
    Ok(prompt.map(|prompt| CodexSessionSummary {
        path: path.to_path_buf(),
        thread_id: thread_id.or_else(|| codex_thread_id_from_path(path)),
        started_at,
        cwd,
        prompt,
        token_usage,
        rate_limits,
        task_complete,
    }))
}

fn codex_session_start_matches_agent(session_started_at: Option<u64>, agent_started_at: u64) -> bool {
    session_started_at
        .is_none_or(|started_at| {
            started_at >= agent_started_at.saturating_sub(300)
                && started_at <= agent_started_at.saturating_add(900)
        })
}

fn codex_session_cwd_matches(session_cwd: Option<&str>, preferred_cwd: Option<&Path>) -> bool {
    let Some(preferred_cwd) = preferred_cwd else {
        return true;
    };
    let Some(session_cwd) = session_cwd else {
        return true;
    };
    Path::new(session_cwd) == preferred_cwd
}

fn token_usage_summary(payload: &Value) -> Option<Value> {
    let info = payload.get("info")?;
    let total = info.get("total_token_usage")?;
    let input = total.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
    let cached = total
        .get("cached_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = total
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let non_cached_input = input.saturating_sub(cached);
    Some(serde_json::json!({
        "input_tokens": input,
        "cached_input_tokens": cached,
        "non_cached_input_tokens": non_cached_input,
        "output_tokens": output,
        "reasoning_output_tokens": total
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        "total_tokens": total
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(input + output),
        "displayed_total_tokens": non_cached_input + output,
        "last_token_usage": info.get("last_token_usage").cloned(),
        "model_context_window": info.get("model_context_window").cloned(),
    }))
}

fn response_item_text(payload: &Value) -> Option<String> {
    let content = payload.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("input_text"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .filter(|text| !text.contains("<environment_context>"))
        .collect::<Vec<_>>()
        .join(" ");
    if text.trim().is_empty() {
        None
    } else {
        Some(text.trim().to_string())
    }
}

fn is_meaningful_task_prompt(message: &str) -> bool {
    let text = message.trim();
    !text.is_empty()
        && text.chars().count() >= 5
        && !text.starts_with('/')
        && !text.starts_with("Token usage:")
        && !text.starts_with("To continue this session")
}

fn codex_thread_id_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
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

fn parse_rfc3339_unix(value: &str) -> Option<u64> {
    let (date, time) = value.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i64>().ok()?;
    let month = date_parts.next()?.parse::<i64>().ok()?;
    let day = date_parts.next()?.parse::<i64>().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let time = time.split_once('.').map_or(time, |(whole, _)| whole);
    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse::<u64>().ok()?;
    let minute = time_parts.next()?.parse::<u64>().ok()?;
    let second = time_parts.next()?.parse::<u64>().ok()?;
    if time_parts.next().is_some()
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    Some(days.cast_unsigned() * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn modified_unix(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn process_running(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}
