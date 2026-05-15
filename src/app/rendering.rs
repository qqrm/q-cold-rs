fn render_task_record(record: &state::TaskRecordRow) -> String {
    format!(
        "task-record\t{}\tsequence={}\tstatus={}\tsource={}\ttitle={}\trepo={}\tcwd={}\tagent={}\tupdated_at={}",
        record.id,
        record.sequence.map(|value| value.to_string()).unwrap_or_default(),
        record.status,
        record.source,
        record.title,
        record.repo_root.as_deref().unwrap_or(""),
        record.cwd.as_deref().unwrap_or(""),
        record.agent_id.as_deref().unwrap_or(""),
        record.updated_at
    )
}

fn render_task_record_token_usage(record: &state::TaskRecordRow) -> Option<String> {
    let metadata = record
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())?;
    render_token_usage(metadata.get("token_usage")?)
}

fn render_task_record_token_efficiency(record: &state::TaskRecordRow) -> Option<String> {
    let metadata = record
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())?;
    render_token_efficiency(metadata.get("token_efficiency")?)
}

fn render_task_record_top_tool_outputs(record: &state::TaskRecordRow) -> Vec<String> {
    let Some(metadata) = record
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    else {
        return Vec::new();
    };
    render_top_tool_outputs(
        metadata
            .get("token_efficiency")
            .and_then(|efficiency| efficiency.get("top_tool_outputs")),
    )
}

fn render_token_usage(usage: &Value) -> Option<String> {
    let object = usage.as_object()?;
    let field = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_u64)
            .unwrap_or_default()
    };
    let context = object
        .get("model_context_window")
        .and_then(Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_default();
    let source = object
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(format!(
        "token-usage\tinput={}\tcached_input={}\tnon_cached_input={}\toutput={}\treasoning={}\t\
         total={}\tdisplayed={}\tmodel_calls={}\tcontext={}\tsource={}",
        field("input_tokens"),
        field("cached_input_tokens"),
        field("non_cached_input_tokens"),
        field("output_tokens"),
        field("reasoning_output_tokens"),
        field("total_tokens"),
        field("displayed_total_tokens"),
        field("model_calls"),
        context,
        source
    ))
}

fn render_token_efficiency(efficiency: &Value) -> Option<String> {
    let object = efficiency.as_object()?;
    let field = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_u64)
            .unwrap_or_default()
    };
    Some(format!(
        "token-efficiency\tsessions={}\tmatched_explicit={}\tmatched_worktree={}\
         \tmatched_task={}\
         \ttool_output_tokens={}\tlarge_tool_outputs={}\tlarge_tool_output_tokens={}\
         \tretention_hours={}\tsource={}",
        field("session_count"),
        field("matched_by_explicit"),
        field("matched_by_worktree"),
        field("matched_by_task"),
        field("tool_output_original_tokens"),
        field("large_tool_output_calls"),
        field("large_tool_output_original_tokens"),
        field("retention_hours"),
        object
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default()
    ))
}

fn render_top_tool_outputs(samples: Option<&Value>) -> Vec<String> {
    let Some(samples) = samples.and_then(Value::as_array) else {
        return Vec::new();
    };
    samples
        .iter()
        .filter_map(|sample| {
            let object = sample.as_object()?;
            Some(format!(
                "token-efficiency-top\toriginal_tokens={}\tsession={}\tcommand={}",
                object
                    .get("original_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                object.get("session").and_then(Value::as_str).unwrap_or(""),
                object.get("command").and_then(Value::as_str).unwrap_or("")
            ))
        })
        .collect()
}

fn agent_command_payload(command: &[String]) -> String {
    match command {
        [tmux, new_session, flag, _session, wrapped, ..]
            if tmux == "tmux" && new_session == "new-session" && flag == "-s" =>
        {
            wrapped.clone()
        }
        [zellij, session_flag, _session, pane_marker, _pane, wrapped, ..]
            if zellij == "zellij" && session_flag == "--session" && pane_marker == "pane" =>
        {
            wrapped.clone()
        }
        _ => command.join(" "),
    }
}

fn is_queue_agent_track(track: &str) -> bool {
    track.strip_prefix("queue-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    })
}

fn prompt_from_agent_command(command: &str) -> Option<String> {
    let lower = command.to_lowercase();
    if !(lower.contains("c1")
        || lower.contains("cc1")
        || lower.contains("c2")
        || lower.contains("cc2")
        || lower.contains("codex"))
    {
        return None;
    }
    let words = shell_words(command);
    let prompt = words
        .iter()
        .rev()
        .find(|word| {
            let clean = word.trim();
            clean.len() >= 3
                && !clean.starts_with('-')
                && clean != "/home/qqrm/.local/bin/c1"
                && clean != "/home/qqrm/.local/bin/cc1"
                && clean != "/home/qqrm/.local/bin/c2"
                && clean != "/home/qqrm/.local/bin/cc2"
                && clean != "c1"
                && clean != "cc1"
                && clean != "c2"
                && clean != "cc2"
                && clean != "codex"
        })?.clone();
    Some(prompt)
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

fn polish_task_text(value: &str) -> String {
    let mut text = value
        .replace(['\n', '\r', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for banned in [
        "блядь",
        "блять",
        "ёб",
        "еб",
        "ебан",
        "ёбан",
        "нахуй",
        "хуй",
        "пизд",
        "сука",
        "уебище",
        "уёбище",
        "fuck",
        "fucking",
        "shit",
    ] {
        text = replace_case_insensitive(&text, banned, "");
    }
    text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(", ,", ",")
        .replace(" ,", ",")
        .trim_matches(|ch: char| ch == ',' || ch == '.' || ch == ';' || ch == ':')
        .trim()
        .to_string();
    if text.is_empty() {
        "Task requested through Q-COLD.".to_string()
    } else {
        text
    }
}

fn replace_case_insensitive(input: &str, needle: &str, replacement: &str) -> String {
    let mut output = String::new();
    let mut rest = input;
    let needle_lower = needle.to_lowercase();
    while let Some(index) = rest.to_lowercase().find(&needle_lower) {
        output.push_str(&rest[..index]);
        output.push_str(replacement);
        rest = &rest[index + needle.len()..];
    }
    output.push_str(rest);
    output
}

fn title_from_slug(slug: &str) -> String {
    let title = slug
        .split(['-', '_', '/'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        "Q-COLD Task".to_string()
    } else {
        title
    }
}

fn title_from_description(description: &str) -> String {
    let words = description
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if words.is_empty() {
        "Q-COLD Task".to_string()
    } else {
        words
            .trim_matches(|ch: char| ch == ',' || ch == '.' || ch == ';' || ch == ':')
            .to_string()
    }
}

fn slug_from_title(title: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in title.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug.chars().take(64).collect()
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn adapter_for_active_repo() -> Result<adapter::XtaskProcessAdapter> {
    adapter_for_context(AdapterContext::ActiveRepository)
}

fn adapter_for_cwd_sensitive_repo() -> Result<adapter::XtaskProcessAdapter> {
    adapter_for_context(AdapterContext::CwdManagedWorktree)
}

fn adapter_for_context(context: AdapterContext) -> Result<adapter::XtaskProcessAdapter> {
    let repo = repository::for_adapter_context(context)?;
    repository_adapter_for(&repo)
}

fn repository_adapter_for(repo: &RepositoryConfig) -> Result<adapter::XtaskProcessAdapter> {
    if repo.adapter != "xtask-process" {
        anyhow::bail!(
            "repository {} uses unsupported adapter {}; supported adapter: xtask-process",
            repo.id,
            repo.adapter
        );
    }
    adapter::xtask_process_for(&repo.root, repo.xtask_manifest.as_deref())
}
