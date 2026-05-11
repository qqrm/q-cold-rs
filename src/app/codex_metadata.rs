#[allow(clippy::too_many_lines, reason = "existing telemetry sync debt")]
pub(crate) fn sync_codex_task_records() -> Result<usize> {
    let agent_rows = state::load_agents(&agents::registry_path()?)?;
    let preferred_cwd = repository::active_root()
        .ok()
        .or_else(|| std::env::current_dir().ok());
    let mut synced = 0;
    let mut existing = state::load_task_records(None, 1000)?;

    synced += sync_task_flow_records(&existing)?;
    existing = state::load_task_records(None, 1000)?;
    let mut claimed_session_paths = claimed_codex_session_paths(&existing);

    for agent in agent_rows {
        if is_queue_agent_track(&agent.track) {
            continue;
        }
        let command = agent_command_payload(&agent.command);
        let Some(account) = codex_account_from_agent_command(&command) else {
            continue;
        };
        let existing_record = existing
            .iter()
            .find(|record| record.agent_id.as_deref() == Some(agent.id.as_str()));
        let mut available_session_paths = claimed_session_paths.clone();
        if let Some(path) = existing_record.and_then(codex_session_path_from_task_record) {
            available_session_paths.remove(&path);
        }
        let agent_preferred_cwd = agent.cwd.as_deref().or(preferred_cwd.as_deref());
        let summary =
            if let Some(path) = existing_record.and_then(codex_session_path_from_task_record) {
                parse_codex_session_summary(Path::new(&path))?
            } else {
                find_codex_session_summary(
                    &account,
                    agent.started_at,
                    &available_session_paths,
                    agent_preferred_cwd,
                )?
            };
        let Some(summary) = summary else {
            continue;
        };
        let description = polish_task_text(&summary.prompt);
        if description.is_empty() {
            continue;
        }

        let title = existing_record
            .map_or_else(|| title_from_description(&description), |record| record.title.clone());
        let record_id = existing_record.map_or_else(
            || format!("adhoc/{}-{}", agent.started_at, slug_from_title(&title)),
            |record| record.id.clone(),
        );
        let status = existing_record.map_or_else(|| {
                if summary.task_complete || !process_running(agent.pid) {
                    "closed:unknown".to_string()
                } else {
                    "open".to_string()
                }
            }, |record| record.status.clone());
        let source = existing_record.map_or_else(|| "codex-session".to_string(), |record| record.source.clone());
        let record_description = existing_record
            .map(|record| record.description.clone())
            .unwrap_or(description);
        let cwd = agent.cwd.clone().or_else(|| std::env::current_dir().ok());
        let metadata = serde_json::json!({
            "kind": "codex-session-import",
            "agent_id": agent.id.clone(),
            "track": agent.track.clone(),
            "command": command,
            "codex_account": account,
            "session_path": summary.path.display().to_string(),
            "codex_thread_id": summary.thread_id,
            "codex_started_at": summary.started_at,
            "codex_cwd": summary.cwd,
            "token_usage": summary.token_usage,
            "rate_limits": summary.rate_limits,
            "task_complete": summary.task_complete,
        });
        let metadata_json = metadata.to_string();
        if let Some(existing_record) = existing_record {
            if existing_record.source == source
                && existing_record.title == title
                && existing_record.description == record_description
                && existing_record.status == status
                && existing_record.metadata_json.as_deref() == Some(metadata_json.as_str())
            {
                continue;
            }
        }
        let record = state::new_task_record(
            record_id,
            source,
            title,
            record_description,
            status,
            repository::active_root()
                .ok()
                .map(|path| path.display().to_string()),
            cwd.map(|path| path.display().to_string()),
            Some(agent.id),
            Some(metadata_json),
        );
        let stored = state::upsert_task_record(&record)?;
        if let Some(path) = codex_session_path_from_task_record(&stored) {
            claimed_session_paths.insert(path);
        }
        synced += 1;
    }

    Ok(synced)
}

#[derive(Debug)]
struct CodexSessionSummary {
    path: PathBuf,
    thread_id: Option<String>,
    started_at: Option<u64>,
    cwd: Option<String>,
    prompt: String,
    token_usage: Option<Value>,
    rate_limits: Option<Value>,
    task_complete: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CodexTokenUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
    model_calls: u64,
    model_context_window: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CodexToolOutputStats {
    calls: u64,
    original_tokens: u64,
    large_calls: u64,
    large_original_tokens: u64,
    samples: Vec<CodexToolOutputSample>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexToolOutputSample {
    original_tokens: u64,
    session: String,
    command: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CodexTaskTelemetry {
    usage: CodexTokenUsage,
    session_ids: Vec<String>,
    session_paths: Vec<String>,
    session_count: u64,
    matched_by_worktree: u64,
    matched_by_task: u64,
    retention_hours: u64,
    tool_outputs: CodexToolOutputStats,
}

impl CodexToolOutputStats {
    fn add_sample(&mut self, sample: CodexToolOutputSample) {
        self.calls += 1;
        self.original_tokens += sample.original_tokens;
        if sample.original_tokens >= LARGE_TOOL_OUTPUT_TOKEN_THRESHOLD {
            self.large_calls += 1;
            self.large_original_tokens += sample.original_tokens;
        }
        self.samples.push(sample);
        self.samples
            .sort_by_key(|sample| Reverse(sample.original_tokens));
        self.samples.truncate(MAX_TOOL_OUTPUT_SAMPLES);
    }
}

impl CodexTaskTelemetry {
    fn efficiency_json(&self, captured_at: u64, started_at: u64, finished_at: u64) -> Value {
        serde_json::json!({
            "source": "codex-session-window",
            "schema_version": 1,
            "captured_at": captured_at,
            "window_started_at": started_at,
            "window_finished_at": finished_at,
            "retention_hours": self.retention_hours,
            "session_count": self.session_count,
            "matched_by_worktree": self.matched_by_worktree,
            "matched_by_task": self.matched_by_task,
            "tool_output_calls": self.tool_outputs.calls,
            "tool_output_original_tokens": self.tool_outputs.original_tokens,
            "large_tool_output_threshold": LARGE_TOOL_OUTPUT_TOKEN_THRESHOLD,
            "large_tool_output_calls": self.tool_outputs.large_calls,
            "large_tool_output_original_tokens": self.tool_outputs.large_original_tokens,
            "top_tool_outputs": self.tool_outputs.samples.iter().map(|sample| {
                serde_json::json!({
                    "original_tokens": sample.original_tokens,
                    "session": sample.session,
                    "command": sample.command,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

impl CodexTokenUsage {
    fn add_last_usage(&mut self, info: &Value) -> bool {
        let Some(last) = info.get("last_token_usage") else {
            return false;
        };
        let input = last
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cached = last
            .get("cached_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = last
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let reasoning = last
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total = last
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(input + output);
        self.input_tokens += input;
        self.cached_input_tokens += cached;
        self.output_tokens += output;
        self.reasoning_output_tokens += reasoning;
        self.total_tokens += total;
        self.model_calls += 1;
        if self.model_context_window.is_none() {
            self.model_context_window = info.get("model_context_window").and_then(Value::as_u64);
        }
        true
    }

    fn as_json(self) -> Value {
        let non_cached_input = self.input_tokens.saturating_sub(self.cached_input_tokens);
        serde_json::json!({
            "input_tokens": self.input_tokens,
            "cached_input_tokens": self.cached_input_tokens,
            "non_cached_input_tokens": non_cached_input,
            "output_tokens": self.output_tokens,
            "reasoning_output_tokens": self.reasoning_output_tokens,
            "total_tokens": self.total_tokens,
            "displayed_total_tokens": non_cached_input + self.output_tokens,
            "model_calls": self.model_calls,
            "model_context_window": self.model_context_window,
            "source": "codex-session-window",
        })
    }
}

fn claimed_codex_session_paths(records: &[state::TaskRecordRow]) -> HashSet<String> {
    records
        .iter()
        .filter_map(codex_session_path_from_task_record)
        .collect()
}

fn codex_session_path_from_task_record(record: &state::TaskRecordRow) -> Option<String> {
    let metadata = serde_json::from_str::<Value>(record.metadata_json.as_deref()?).ok()?;
    metadata
        .get("session_path")
        .and_then(Value::as_str)
        .map(str::to_string)
}
