enum QueueAgentSelection {
    Selected {
        command: String,
        record: Box<AgentLimitRecord>,
    },
    Waiting {
        message: String,
        next_retry_at: u64,
    },
}

fn select_queue_agent_for_launch(now: u64) -> QueueAgentSelection {
    let cache = AGENT_LIMIT_CACHE.get_or_init(|| Mutex::new(None));
    let cached = cache.lock().ok().and_then(|guard| guard.clone());
    let stale = cached
        .as_ref()
        .is_none_or(|cached| now >= cached.generated_at_unix.saturating_add(AGENT_LIMIT_CACHE_TTL));
    if stale {
        schedule_agent_limit_refresh();
    }
    if let Some(cached) = cached {
        return select_queue_agent_from_records(now, &cached.records);
    }
    QueueAgentSelection::Waiting {
        message: "waiting for c1/c2 status probe".to_string(),
        next_retry_at: now.saturating_add(AGENT_LIMIT_PENDING_RETRY),
    }
}

fn select_queue_agent_from_records(now: u64, records: &[AgentLimitRecord]) -> QueueAgentSelection {
    if let Some(record) = records
        .iter()
        .filter(|record| queue_agent_record_usable(record, now))
        .max_by_key(|record| (record.capacity_score, std::cmp::Reverse(record.command.clone())))
    {
        return QueueAgentSelection::Selected {
            command: record.command.clone(),
            record: Box::new(record.clone()),
        };
    }
    let next_retry_at = records
        .iter()
        .filter_map(queue_agent_next_retry_at)
        .filter(|retry_at| *retry_at > now)
        .min()
        .unwrap_or_else(|| now.saturating_add(AGENT_LIMIT_PENDING_RETRY));
    let message = if records.is_empty() {
        "no eligible c1/c2 agent command is available".to_string()
    } else {
        format!(
            "all eligible c1/c2 agents are waiting; {}",
            agent_limit_summary(records)
        )
    };
    QueueAgentSelection::Waiting {
        message,
        next_retry_at,
    }
}

fn queue_agent_record_usable(record: &AgentLimitRecord, now: u64) -> bool {
    queue_agent_selector_command(&record.command)
        && record.state == "ok"
        && record.capacity_score > 0
        && now < record.expires_at_unix
}

fn queue_agent_next_retry_at(record: &AgentLimitRecord) -> Option<u64> {
    record.reset_at_unix.or(Some(record.expires_at_unix))
}

fn agent_limit_summary(records: &[AgentLimitRecord]) -> String {
    records
        .iter()
        .filter(|record| queue_agent_selector_command(&record.command))
        .map(|record| {
            let retry = record
                .reset_at_unix
                .or(Some(record.expires_at_unix))
                .map(|value| format!(" retry_at={value}"))
                .unwrap_or_default();
            format!(
                "{} state={} capacity={}{} summary={}",
                record.command, record.state, record.capacity_score, retry, record.summary
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn retry_index(retries: i64) -> usize {
    usize::try_from(retries).unwrap_or(usize::MAX)
}
