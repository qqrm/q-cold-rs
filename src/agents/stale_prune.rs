#[derive(Default)]
struct StalePruneSummary {
    max_age_hours: u64,
    dry_run: bool,
    matched_agents: usize,
    terminated_agents: usize,
    deleted_agents: usize,
    deleted_task_records: usize,
    skipped_attached: usize,
    skipped_unknown_clients: usize,
    events: Vec<StalePruneEvent>,
}

impl StalePruneSummary {
    fn render(&self) -> String {
        format!(
            "agent-prune\tmax_age_hours={}\tdry_run={}\tterminated_agents={}\tdeleted_agents={}\
             \tdeleted_task_records={}\tmatched_agents={}\tskipped_attached={}\
             \tskipped_unknown_clients={}",
            self.max_age_hours,
            self.dry_run,
            self.terminated_agents,
            self.deleted_agents,
            self.deleted_task_records,
            self.matched_agents,
            self.skipped_attached,
            self.skipped_unknown_clients,
        )
    }
}

struct StalePruneEvent {
    id: String,
    action: &'static str,
    reason: &'static str,
}

impl StalePruneEvent {
    fn render(&self) -> String {
        format!(
            "agent-prune-row\tid={}\taction={}\treason={}",
            self.id, self.action, self.reason
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StaleAgentDecision {
    DeleteExited,
    TerminateUnattached,
    TerminateAttached,
    SkipRecent,
    SkipAttached,
    SkipRunningPlain,
    SkipUnknownClients,
}

fn prune_stale_agents_best_effort() {
    let Ok(max_age_hours) = agent_stale_ttl_hours() else {
        return;
    };
    let _ = prune_stale_agents(max_age_hours, false, false);
}

fn prune_stale_agents(
    max_age_hours: u64,
    include_attached: bool,
    dry_run: bool,
) -> Result<StalePruneSummary> {
    let records = AgentState::load()?.records;
    let now = unix_now()?;
    let max_age_seconds = max_age_hours.saturating_mul(60 * 60);
    let activity = agent_task_record_updates()?;
    let mut summary = StalePruneSummary {
        max_age_hours,
        dry_run,
        ..StalePruneSummary::default()
    };
    let mut delete_agent_ids = Vec::new();

    for record in records {
        let state = process_state(record.pid);
        let target = terminal_target(&record);
        let client_state = target
            .as_ref()
            .map(terminal_target_has_clients)
            .transpose()
            .unwrap_or(None);
        let latest_activity = activity
            .get(&record.id)
            .copied()
            .unwrap_or(record.started_at)
            .max(record.started_at);
        match stale_agent_decision(
            state,
            target.is_some(),
            client_state,
            now,
            max_age_seconds,
            latest_activity,
            include_attached,
        ) {
            StaleAgentDecision::DeleteExited => {
                summary.matched_agents += 1;
                delete_agent_ids.push(record.id.clone());
                summary.events.push(StalePruneEvent {
                    id: record.id,
                    action: "delete-record",
                    reason: "exited-stale",
                });
            }
            StaleAgentDecision::TerminateUnattached | StaleAgentDecision::TerminateAttached => {
                summary.matched_agents += 1;
                let reason = if client_state == Some(true) {
                    "attached-stale"
                } else {
                    "unattached-stale"
                };
                if let Some(target) = target.as_ref() {
                    if !dry_run {
                        terminate_terminal_target(target)?;
                        let key = terminal_target_key_for_metadata(target);
                        let _ = state::save_terminal_metadata(&key, None, None);
                    }
                    summary.terminated_agents += 1;
                    delete_agent_ids.push(record.id.clone());
                    summary.events.push(StalePruneEvent {
                        id: record.id,
                        action: "terminate-delete-record",
                        reason,
                    });
                }
            }
            StaleAgentDecision::SkipAttached => summary.skipped_attached += 1,
            StaleAgentDecision::SkipUnknownClients => summary.skipped_unknown_clients += 1,
            StaleAgentDecision::SkipRecent | StaleAgentDecision::SkipRunningPlain => {}
        }
    }

    if !dry_run {
        for id in &delete_agent_ids {
            summary.deleted_task_records += state::delete_ad_hoc_task_records_for_agent(id)?;
            if state::delete_agent_record(id)? {
                summary.deleted_agents += 1;
            }
        }
    }

    Ok(summary)
}

fn stale_agent_decision(
    process_state: &str,
    has_terminal: bool,
    has_clients: Option<bool>,
    now: u64,
    max_age_seconds: u64,
    latest_activity: u64,
    include_attached: bool,
) -> StaleAgentDecision {
    if now.saturating_sub(latest_activity) < max_age_seconds {
        return StaleAgentDecision::SkipRecent;
    }
    if process_state != "running" {
        return StaleAgentDecision::DeleteExited;
    }
    if !has_terminal {
        return StaleAgentDecision::SkipRunningPlain;
    }
    match has_clients {
        Some(false) => StaleAgentDecision::TerminateUnattached,
        Some(true) if include_attached => StaleAgentDecision::TerminateAttached,
        Some(true) => StaleAgentDecision::SkipAttached,
        None => StaleAgentDecision::SkipUnknownClients,
    }
}

fn agent_stale_ttl_hours() -> Result<u64> {
    match env::var("QCOLD_AGENT_STALE_TTL_HOURS") {
        Ok(value) => value
            .parse::<u64>()
            .with_context(|| format!("invalid QCOLD_AGENT_STALE_TTL_HOURS={value}")),
        Err(_) => Ok(DEFAULT_AGENT_STALE_TTL_HOURS),
    }
}

fn agent_task_record_updates() -> Result<HashMap<String, u64>> {
    let mut updates = HashMap::new();
    for record in state::load_task_records(None, 10_000)? {
        let Some(agent_id) = record.agent_id else {
            continue;
        };
        updates
            .entry(agent_id)
            .and_modify(|updated_at: &mut u64| *updated_at = (*updated_at).max(record.updated_at))
            .or_insert(record.updated_at);
    }
    Ok(updates)
}
