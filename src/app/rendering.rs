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

fn render_task_record_audit(records: &[state::TaskRecordRow], top_limit: usize) -> Vec<String> {
    let mut total = TaskRecordAuditBucket::default();
    let mut by_source = BTreeMap::<String, TaskRecordAuditBucket>::new();
    let mut gaps = BTreeMap::<(String, String, String), usize>::new();
    let mut ranked = Vec::<TaskRecordAuditEntry>::new();

    for record in records {
        let metrics = task_record_audit_metrics(record);
        total.add(record, &metrics);
        by_source
            .entry(record.source.clone())
            .or_default()
            .add(record, &metrics);
        if !metrics.has_token_usage {
            *gaps
                .entry((
                    "missing-token-usage".to_string(),
                    record.source.clone(),
                    record.status.clone(),
                ))
                .or_default() += 1;
        }
        if !metrics.has_token_efficiency {
            *gaps
                .entry((
                    "missing-token-efficiency".to_string(),
                    record.source.clone(),
                    record.status.clone(),
                ))
                .or_default() += 1;
        }
        ranked.push(TaskRecordAuditEntry::from_record(record, metrics));
    }

    let mut lines = vec![format!("task-record-audit\t{}", total.render_summary())];
    for (source, bucket) in by_source {
        lines.push(format!(
            "task-record-audit-source\tsource={}\t{}",
            audit_text(&source, 80),
            bucket.render_summary()
        ));
    }
    for ((kind, source, status), count) in gaps {
        lines.push(format!(
            "task-record-audit-gap\tkind={}\tsource={}\tstatus={}\tcount={}",
            kind,
            audit_text(&source, 80),
            audit_text(&status, 80),
            count
        ));
    }

    ranked.sort_by_key(|entry| Reverse(entry.metrics.tool_output_tokens));
    for (index, entry) in ranked
        .iter()
        .filter(|entry| entry.metrics.tool_output_tokens > 0)
        .take(top_limit)
        .enumerate()
    {
        lines.push(entry.render("task-record-audit-top-tool", index + 1));
    }

    ranked.sort_by_key(|entry| Reverse(entry.metrics.total_tokens));
    for (index, entry) in ranked
        .iter()
        .filter(|entry| entry.metrics.total_tokens > 0)
        .take(top_limit)
        .enumerate()
    {
        lines.push(entry.render("task-record-audit-top-cost", index + 1));
    }

    lines
}

#[derive(Default)]
struct TaskRecordAuditBucket {
    records: usize,
    token_usage_records: usize,
    token_efficiency_records: usize,
    open_records: usize,
    closed_success_records: usize,
    closed_blocked_records: usize,
    closed_failed_records: usize,
    closed_unknown_records: usize,
    total_tokens: u64,
    displayed_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    model_calls: u64,
    sessions: u64,
    tool_output_tokens: u64,
    large_tool_outputs: u64,
    large_tool_output_tokens: u64,
}

impl TaskRecordAuditBucket {
    fn add(&mut self, record: &state::TaskRecordRow, metrics: &TaskRecordAuditMetrics) {
        self.records += 1;
        self.token_usage_records += usize::from(metrics.has_token_usage);
        self.token_efficiency_records += usize::from(metrics.has_token_efficiency);
        match record.status.as_str() {
            "open" => self.open_records += 1,
            "closed:success" => self.closed_success_records += 1,
            "closed:blocked" => self.closed_blocked_records += 1,
            "closed:failed" => self.closed_failed_records += 1,
            "closed:unknown" => self.closed_unknown_records += 1,
            _ => {}
        }
        self.total_tokens = self.total_tokens.saturating_add(metrics.total_tokens);
        self.displayed_tokens = self.displayed_tokens.saturating_add(metrics.displayed_tokens);
        self.output_tokens = self.output_tokens.saturating_add(metrics.output_tokens);
        self.reasoning_tokens = self
            .reasoning_tokens
            .saturating_add(metrics.reasoning_tokens);
        self.model_calls = self.model_calls.saturating_add(metrics.model_calls);
        self.sessions = self.sessions.saturating_add(metrics.sessions);
        self.tool_output_tokens = self
            .tool_output_tokens
            .saturating_add(metrics.tool_output_tokens);
        self.large_tool_outputs = self
            .large_tool_outputs
            .saturating_add(metrics.large_tool_outputs);
        self.large_tool_output_tokens = self
            .large_tool_output_tokens
            .saturating_add(metrics.large_tool_output_tokens);
    }

    fn render_summary(&self) -> String {
        format!(
            concat!(
                "records={}\ttoken_usage_records={}\ttoken_efficiency_records={}",
                "\tmissing_token_usage={}\tmissing_token_efficiency={}",
                "\topen={}\tclosed_success={}\tclosed_blocked={}\tclosed_failed={}",
                "\tclosed_unknown={}\ttotal_tokens={}\tdisplayed_tokens={}",
                "\toutput_tokens={}\treasoning_tokens={}\tmodel_calls={}\tsessions={}",
                "\ttool_output_tokens={}\tlarge_tool_outputs={}",
                "\tlarge_tool_output_tokens={}\ttool_output_ratio_ppm={}",
                "\tlarge_output_ratio_ppm={}",
            ),
            self.records,
            self.token_usage_records,
            self.token_efficiency_records,
            self.records.saturating_sub(self.token_usage_records),
            self.records.saturating_sub(self.token_efficiency_records),
            self.open_records,
            self.closed_success_records,
            self.closed_blocked_records,
            self.closed_failed_records,
            self.closed_unknown_records,
            self.total_tokens,
            self.displayed_tokens,
            self.output_tokens,
            self.reasoning_tokens,
            self.model_calls,
            self.sessions,
            self.tool_output_tokens,
            self.large_tool_outputs,
            self.large_tool_output_tokens,
            ratio_ppm(self.tool_output_tokens, self.total_tokens),
            ratio_ppm(self.large_tool_output_tokens, self.tool_output_tokens)
        )
    }
}

#[derive(Clone, Default)]
struct TaskRecordAuditMetrics {
    has_token_usage: bool,
    has_token_efficiency: bool,
    total_tokens: u64,
    displayed_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    model_calls: u64,
    sessions: u64,
    tool_output_tokens: u64,
    large_tool_outputs: u64,
    large_tool_output_tokens: u64,
}

struct TaskRecordAuditEntry {
    id: String,
    title: String,
    source: String,
    status: String,
    repo: String,
    metrics: TaskRecordAuditMetrics,
}

impl TaskRecordAuditEntry {
    fn from_record(record: &state::TaskRecordRow, metrics: TaskRecordAuditMetrics) -> Self {
        Self {
            id: record.id.clone(),
            title: record.title.clone(),
            source: record.source.clone(),
            status: record.status.clone(),
            repo: record.repo_root.clone().unwrap_or_default(),
            metrics,
        }
    }

    fn render(&self, kind: &str, rank: usize) -> String {
        format!(
            concat!(
                "{}\trank={}\ttotal_tokens={}\ttool_output_tokens={}",
                "\tlarge_tool_output_tokens={}\tlarge_tool_outputs={}",
                "\tmodel_calls={}\tsource={}\tstatus={}\trepo={}\tid={}\ttitle={}",
            ),
            kind,
            rank,
            self.metrics.total_tokens,
            self.metrics.tool_output_tokens,
            self.metrics.large_tool_output_tokens,
            self.metrics.large_tool_outputs,
            self.metrics.model_calls,
            audit_text(&self.source, 80),
            audit_text(&self.status, 80),
            audit_text(&self.repo, 120),
            audit_text(&self.id, 120),
            audit_text(&self.title, 120)
        )
    }
}

fn task_record_audit_metrics(record: &state::TaskRecordRow) -> TaskRecordAuditMetrics {
    let Some(metadata) = record
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    else {
        return TaskRecordAuditMetrics::default();
    };
    let mut metrics = TaskRecordAuditMetrics::default();
    if let Some(usage) = metadata.get("token_usage").and_then(Value::as_object) {
        metrics.has_token_usage = true;
        metrics.total_tokens = audit_u64(usage, "total_tokens");
        metrics.displayed_tokens = audit_u64(usage, "displayed_total_tokens");
        metrics.output_tokens = audit_u64(usage, "output_tokens");
        metrics.reasoning_tokens = audit_u64(usage, "reasoning_output_tokens");
        metrics.model_calls = audit_u64(usage, "model_calls");
    }
    if let Some(efficiency) = metadata
        .get("token_efficiency")
        .and_then(Value::as_object)
    {
        metrics.has_token_efficiency = true;
        metrics.sessions = audit_u64(efficiency, "session_count");
        metrics.tool_output_tokens = audit_u64(efficiency, "tool_output_original_tokens");
        metrics.large_tool_outputs = audit_u64(efficiency, "large_tool_output_calls");
        metrics.large_tool_output_tokens = audit_u64(efficiency, "large_tool_output_original_tokens");
    }
    metrics
}

fn audit_u64(object: &serde_json::Map<String, Value>, name: &str) -> u64 {
    object.get(name).and_then(Value::as_u64).unwrap_or_default()
}

fn ratio_ppm(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return 0;
    }
    numerator.saturating_mul(1_000_000) / denominator
}

fn audit_text(value: &str, max_chars: usize) -> String {
    let compact = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\t', " ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    compact.chars().take(max_chars).collect()
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

fn adapter_for_task_flow_repo() -> Result<adapter::XtaskProcessAdapter> {
    adapter_for_context(AdapterContext::TaskFlowRepository)
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
    adapter::xtask_process_for(
        &repo.root,
        repo.xtask_manifest.as_deref(),
        repo.default_branch.as_deref(),
    )
}
