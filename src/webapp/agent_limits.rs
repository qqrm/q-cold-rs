#[derive(Clone)]
struct AgentLimitCache {
    generated_at_unix: u64,
    records: Vec<AgentLimitRecord>,
}

#[derive(Serialize)]
struct AgentLimitSnapshot {
    generated_at_unix: u64,
    cached: bool,
    refreshing: bool,
    count: usize,
    records: Vec<AgentLimitRecord>,
}

#[derive(Clone, Serialize)]
struct AgentLimitRecord {
    command: String,
    account: String,
    status_command: String,
    state: String,
    capacity_score: i64,
    reset_at_unix: Option<u64>,
    summary: String,
    detail: String,
    raw_summary: String,
    checked_at_unix: u64,
    expires_at_unix: u64,
    attempts: usize,
}

fn agent_limit_snapshot(refresh: bool) -> AgentLimitSnapshot {
    let now = unix_now();
    let cache = AGENT_LIMIT_CACHE.get_or_init(|| Mutex::new(None));
    let cached = cache.lock().ok().and_then(|guard| guard.clone());
    let stale = cached
        .as_ref()
        .is_none_or(|cached| now >= cached.generated_at_unix.saturating_add(AGENT_LIMIT_CACHE_TTL));
    if refresh || stale {
        schedule_agent_limit_refresh();
    }
    let refreshing = agent_limit_refreshing();
    if let Some(cached) = cached {
        return AgentLimitSnapshot {
            generated_at_unix: cached.generated_at_unix,
            cached: true,
            refreshing,
            count: cached.records.len(),
            records: cached.records,
        };
    }
    let records = pending_agent_limit_records(now);
    AgentLimitSnapshot {
        generated_at_unix: now,
        cached: false,
        refreshing,
        count: records.len(),
        records,
    }
}

fn schedule_agent_limit_refresh() {
    let refreshing = AGENT_LIMIT_REFRESHING.get_or_init(|| Mutex::new(false));
    if let Ok(mut guard) = refreshing.lock() {
        if *guard {
            return;
        }
        *guard = true;
    } else {
        return;
    }
    thread::spawn(|| {
        let records = probe_agent_limits();
        let now = unix_now();
        let cache = AGENT_LIMIT_CACHE.get_or_init(|| Mutex::new(None));
        if let Ok(mut cached) = cache.lock() {
            *cached = Some(AgentLimitCache {
                generated_at_unix: now,
                records,
            });
        }
        if let Ok(mut refreshing) = AGENT_LIMIT_REFRESHING.get_or_init(|| Mutex::new(false)).lock()
        {
            *refreshing = false;
        }
    });
}

fn agent_limit_refreshing() -> bool {
    AGENT_LIMIT_REFRESHING
        .get_or_init(|| Mutex::new(false))
        .lock()
        .is_ok_and(|guard| *guard)
}

fn probe_agent_limits() -> Vec<AgentLimitRecord> {
    let agents = agent_selector_agents();
    let mut probes = BTreeMap::<String, agents::AvailableAgentCommand>::new();
    for agent in &agents {
        probes
            .entry(agent.account.clone())
            .or_insert_with(|| agent.clone());
    }
    let handles = probes
        .into_values()
        .map(|agent| thread::spawn(move || (agent.account.clone(), probe_agent_limit(&agent))))
        .collect::<Vec<_>>();
    let mut by_account = BTreeMap::new();
    for handle in handles {
        if let Ok((account, record)) = handle.join() {
            by_account.insert(account, record);
        }
    }
    agents
        .into_iter()
        .map(|agent| {
            let Some(record) = by_account.get(&agent.account) else {
                return unchecked_agent_limit(&agent, "status probe did not return");
            };
            AgentLimitRecord {
                command: agent.command,
                account: agent.account,
                status_command: agent.status_command,
                state: record.state.clone(),
                capacity_score: record.capacity_score,
                reset_at_unix: record.reset_at_unix,
                summary: record.summary.clone(),
                detail: record.detail.clone(),
                raw_summary: record.raw_summary.clone(),
                checked_at_unix: record.checked_at_unix,
                expires_at_unix: record.expires_at_unix,
                attempts: record.attempts,
            }
        })
        .collect()
}

fn probe_agent_limit(agent: &agents::AvailableAgentCommand) -> AgentLimitRecord {
    if !agent_auth_file(&agent.account).is_file() {
        let checked_at_unix = unix_now();
        return agent_limit_record(
            agent,
            "unauthenticated",
            0,
            None,
            "no auth.json",
            format!("missing {}", agent_auth_file(&agent.account).display()),
            "",
            checked_at_unix,
            0,
        );
    }
    let mut last = None;
    for attempt in 1..=AGENT_LIMIT_STATUS_ATTEMPTS {
        let output = run_agent_status_probe(&agent.status_command);
        let record = classify_agent_status_output(agent, attempt, output);
        if record.state != "retry" {
            return record;
        }
        last = Some(record);
    }
    last.unwrap_or_else(|| unchecked_agent_limit(agent, "status probe was not attempted"))
}

fn run_agent_status_probe(command: &str) -> io::Result<std::process::Output> {
    Command::new("timeout")
        .arg(format!("{AGENT_LIMIT_STATUS_TIMEOUT}s"))
        .arg("script")
        .arg("-q")
        .arg("-c")
        .arg(format!("{} /status", queue_shell_quote(command)))
        .arg("/dev/null")
        .env("QCOLD_AGENT_MANAGED_WORKTREE", "0")
        .stdin(Stdio::null())
        .output()
}

fn classify_agent_status_output(
    agent: &agents::AvailableAgentCommand,
    attempt: usize,
    output: io::Result<std::process::Output>,
) -> AgentLimitRecord {
    let checked_at_unix = unix_now();
    let output = match output {
        Ok(output) => output,
        Err(err) => {
            return agent_limit_record(
                agent,
                "error",
                0,
                None,
                "status probe failed to start",
                err.to_string(),
                "",
                checked_at_unix,
                attempt,
            );
        }
    };
    let text = compact_probe_output(&output);
    classify_agent_status_text(
        agent,
        attempt,
        checked_at_unix,
        output.status.success(),
        output.status.code(),
        &text,
    )
}

fn classify_agent_status_text(
    agent: &agents::AvailableAgentCommand,
    attempt: usize,
    checked_at_unix: u64,
    success: bool,
    status_code: Option<i32>,
    text: &str,
) -> AgentLimitRecord {
    let lower = text.to_lowercase();
    let code = status_code.unwrap_or_default();
    let timed_out = code == 124;
    let explicit_state = normalized_status_state(text);
    let capacity_score = normalized_capacity_score(text, checked_at_unix).unwrap_or_else(|| {
        if success && !status_text_limited(&lower) {
            100
        } else {
            0
        }
    });
    let reset_at_unix = normalized_reset_at_unix(text, checked_at_unix);
    let limited = explicit_state == Some("limited")
        || status_text_limited(&lower)
        || capacity_score == 0 && lower.contains("remaining");
    let transient = lower.contains("try again")
        || lower.contains("temporar")
        || lower.contains("429")
        || timed_out;
    let state = if limited {
        "limited"
    } else if timed_out {
        "timeout"
    } else if let Some(state) = explicit_state {
        state
    } else if success {
        "ok"
    } else if transient && attempt < AGENT_LIMIT_STATUS_ATTEMPTS {
        "retry"
    } else {
        "error"
    };
    let summary = if limited {
        extract_relevant_status_line(text).unwrap_or_else(|| "limit reached".to_string())
    } else if timed_out {
        format!("readiness probe timed out after {AGENT_LIMIT_STATUS_TIMEOUT}s")
    } else if success {
        extract_relevant_status_line(text)
            .unwrap_or_else(|| "readiness probe completed".to_string())
    } else {
        extract_relevant_status_line(text)
            .unwrap_or_else(|| format!("readiness probe exited with code {code}"))
    };
    agent_limit_record(
        agent,
        state,
        capacity_score,
        reset_at_unix,
        summary,
        truncate_chars(text, 600),
        short_raw_status_summary(text),
        checked_at_unix,
        attempt,
    )
}

fn compact_probe_output(output: &std::process::Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    strip_ansi(&text)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(80)
        .collect::<Vec<_>>()
        .join("\n")
}

fn status_text_limited(lower: &str) -> bool {
    lower.contains("usage limit reached")
        || lower.contains("rate limit reached")
        || lower.contains("limit reached")
        || lower.contains("limit exceeded")
        || lower.contains("quota exceeded")
        || lower.contains("0% remaining")
        || lower.contains("0 % remaining")
}

fn normalized_status_state(text: &str) -> Option<&'static str> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        for key in ["state", "status", "limit_state"] {
            if let Some(state) = value
                .get(key)
                .and_then(Value::as_str)
                .and_then(normalized_state_value)
            {
                return Some(state);
            }
        }
    }
    text.lines()
        .filter_map(|line| {
            let lower = line.to_lowercase();
            (lower.contains("state") || lower.contains("status") || lower.contains("limit"))
                .then_some(lower)
        })
        .find_map(|line| normalized_state_value(&line))
}

fn normalized_state_value(value: &str) -> Option<&'static str> {
    let lower = value.to_lowercase();
    if lower.contains("unauth") || lower.contains("not authenticated") {
        return Some("unauthenticated");
    }
    if lower.contains("limited")
        || lower.contains("limit reached")
        || lower.contains("limit exceeded")
        || lower.contains("quota exceeded")
    {
        return Some("limited");
    }
    if lower.contains("timeout") || lower.contains("timed out") {
        return Some("timeout");
    }
    if lower.contains("retry") || lower.contains("try again") {
        return Some("retry");
    }
    if lower.contains("error") || lower.contains("failed") {
        return Some("error");
    }
    if lower.contains("ok")
        || lower.contains("ready")
        || lower.contains("usable")
        || lower.contains("available")
    {
        return Some("ok");
    }
    None
}

fn normalized_capacity_score(text: &str, now: u64) -> Option<i64> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        if let Some(score) = json_capacity_score(&value) {
            return Some(score);
        }
    }
    text.lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            ["capacity", "remaining", "available", "quota"]
                .iter()
                .any(|keyword| lower.contains(keyword))
        })
        .find_map(parse_capacity_score_line)
        .or_else(|| {
            let lower = text.to_lowercase();
            status_text_limited(&lower).then_some(0)
        })
        .map(|score| score.clamp(0, 100))
        .or_else(|| {
            let reset = normalized_reset_at_unix(text, now)?;
            (reset <= now).then_some(0)
        })
}

fn json_capacity_score(value: &Value) -> Option<i64> {
    for key in [
        "capacity_score",
        "capacity",
        "remaining_percent",
        "remaining_pct",
        "remaining",
    ] {
        if let Some(score) = value.get(key).and_then(json_number_to_score) {
            return Some(score);
        }
    }
    None
}

fn json_number_to_score(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return Some(number.clamp(0, 100));
    }
    let number = value.as_f64()?;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "rounded percentage scores are intentionally clamped to the 0..=100 queue capacity range"
    )]
    if (0.0..=1.0).contains(&number) {
        return Some((number * 100.0).round() as i64);
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "rounded percentage scores are intentionally clamped to the 0..=100 queue capacity range"
    )]
    Some((number.round() as i64).clamp(0, 100))
}

fn parse_capacity_score_line(line: &str) -> Option<i64> {
    let words = line
        .split(|ch: char| !ch.is_ascii_digit() && ch != '%')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    for word in &words {
        if let Some(percent) = word.strip_suffix('%') {
            if let Ok(value) = percent.parse::<i64>() {
                return Some(value.clamp(0, 100));
            }
        }
    }
    words
        .iter()
        .filter_map(|word| word.parse::<i64>().ok())
        .find(|value| (0..=100).contains(value))
}

fn normalized_reset_at_unix(text: &str, now: u64) -> Option<u64> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        if let Some(reset) = json_reset_at_unix(&value, now) {
            return Some(reset);
        }
    }
    text.lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            lower.contains("reset") || lower.contains("retry") || lower.contains("try again")
        })
        .find_map(|line| parse_reset_at_line(line, now))
}

fn json_reset_at_unix(value: &Value, now: u64) -> Option<u64> {
    for key in [
        "reset_at_unix",
        "reset_time_unix",
        "reset_at",
        "reset_time",
        "reset",
        "retry_at_unix",
    ] {
        let Some(value) = value.get(key) else {
            continue;
        };
        if let Some(unix) = value.as_u64() {
            return Some(unix);
        }
        if let Some(text) = value.as_str().and_then(|value| parse_reset_at_line(value, now)) {
            return Some(text);
        }
    }
    None
}

fn parse_reset_at_line(line: &str, now: u64) -> Option<u64> {
    if let Some(unix) = line
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| part.len() >= 10)
        .filter_map(|part| part.parse::<u64>().ok())
        .find(|value| *value > 1_500_000_000)
    {
        return Some(unix);
    }
    parse_relative_seconds(line).map(|seconds| now.saturating_add(seconds))
}

fn parse_relative_seconds(line: &str) -> Option<u64> {
    let mut total = 0_u64;
    let mut pending_number = None;
    let mut found = false;
    for token in line.split_whitespace() {
        let token = token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
        if token.is_empty() {
            continue;
        }
        if let Ok(value) = token.parse::<u64>() {
            pending_number = Some(value);
            continue;
        }
        if let Some((value, unit)) = parse_compact_relative_token(token) {
            total = total.saturating_add(relative_seconds(value, unit));
            found = true;
            pending_number = None;
            continue;
        }
        if let Some(value) = pending_number.take() {
            if let Some(unit) = relative_unit(token) {
                total = total.saturating_add(relative_seconds(value, unit));
                found = true;
            }
        }
    }
    found.then_some(total)
}

fn parse_compact_relative_token(token: &str) -> Option<(u64, char)> {
    let split = token
        .char_indices()
        .find_map(|(index, ch)| (!ch.is_ascii_digit()).then_some(index))?;
    let value = token[..split].parse::<u64>().ok()?;
    let unit = relative_unit(&token[split..])?;
    Some((value, unit))
}

fn relative_unit(token: &str) -> Option<char> {
    let lower = token.to_ascii_lowercase();
    if lower.starts_with('h') {
        return Some('h');
    }
    if lower.starts_with('m') {
        return Some('m');
    }
    if lower.starts_with('s') {
        return Some('s');
    }
    None
}

fn relative_seconds(value: u64, unit: char) -> u64 {
    match unit {
        'h' => value.saturating_mul(3600),
        'm' => value.saturating_mul(60),
        's' => value,
        _ => 0,
    }
}

fn short_raw_status_summary(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join(" | ")
        .chars()
        .take(240)
        .collect()
}

fn extract_relevant_status_line(text: &str) -> Option<String> {
    let keywords = [
        "limit",
        "remaining",
        "reset",
        "quota",
        "rate",
        "usage",
        "try again",
        "error",
    ];
    text.lines()
        .find(|line| {
            let lower = line.to_lowercase();
            keywords.iter().any(|keyword| lower.contains(keyword))
        })
        .map(|line| truncate_chars(line.trim(), 160))
}

fn strip_ansi(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            result.push(ch);
            continue;
        }
        for next in chars.by_ref() {
            if next.is_ascii_alphabetic() || matches!(next, '~' | '\\') {
                break;
            }
        }
    }
    result
}

fn unchecked_agent_limit(
    agent: &agents::AvailableAgentCommand,
    summary: impl Into<String>,
) -> AgentLimitRecord {
    let checked_at_unix = unix_now();
    agent_limit_record(
        agent,
        "unknown",
        0,
        None,
        summary.into(),
        "",
        "",
        checked_at_unix,
        0,
    )
}

fn pending_agent_limit_records(now: u64) -> Vec<AgentLimitRecord> {
    agent_selector_agents()
        .into_iter()
        .map(|agent| {
            agent_limit_record(
                &agent,
                "unknown",
                0,
                None,
                "status probe pending",
                "",
                "",
                now,
                0,
            )
        })
        .collect()
}

#[allow(
    clippy::too_many_arguments,
    reason = "central record constructor keeps call sites explicit about derived status probe fields"
)]
fn agent_limit_record(
    agent: &agents::AvailableAgentCommand,
    state: impl Into<String>,
    capacity_score: i64,
    reset_at_unix: Option<u64>,
    summary: impl Into<String>,
    detail: impl Into<String>,
    raw_summary: impl Into<String>,
    checked_at_unix: u64,
    attempts: usize,
) -> AgentLimitRecord {
    AgentLimitRecord {
        command: agent.command.clone(),
        account: agent.account.clone(),
        status_command: agent.status_command.clone(),
        state: state.into(),
        capacity_score: capacity_score.clamp(0, 100),
        reset_at_unix,
        summary: summary.into(),
        detail: detail.into(),
        raw_summary: raw_summary.into(),
        checked_at_unix,
        expires_at_unix: checked_at_unix.saturating_add(AGENT_LIMIT_CACHE_TTL),
        attempts,
    }
}

fn agent_selector_agents() -> Vec<agents::AvailableAgentCommand> {
    agents::available_agent_commands()
        .into_iter()
        .filter(|agent| queue_agent_selector_command(&agent.command))
        .collect()
}

fn queue_agent_selector_command(command: &str) -> bool {
    matches!(command, "c1" | "c2")
}

fn agent_auth_file(account: &str) -> PathBuf {
    agents::agent_auth_file(account)
}
