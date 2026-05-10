use anyhow::Result;

pub use crate::state::HistoryEntry;

pub fn append(source: &str, role: &str, text: &str) -> Result<()> {
    crate::state::append_history(source, role, text)
}

pub fn load_recent(limit: usize) -> Result<Vec<HistoryEntry>> {
    crate::state::load_history(limit)
}

pub fn load_recent_for_source(source: &str, limit: usize) -> Result<Vec<HistoryEntry>> {
    crate::state::load_history_for_source(source, limit)
}

pub fn load_recent_meta_visible(limit: usize) -> Result<Vec<HistoryEntry>> {
    Ok(visible_entries(load_recent(expanded_limit(limit))?, limit))
}

pub fn load_recent_meta_visible_for_source(source: &str, limit: usize) -> Result<Vec<HistoryEntry>> {
    Ok(visible_entries(
        load_recent_for_source(source, expanded_limit(limit))?,
        limit,
    ))
}

pub fn prompt_context(current_text: &str, limit: usize) -> Result<String> {
    let entries = load_recent_meta_visible(limit)?;
    let mut lines = vec![
        "Q-COLD shared operator history follows. Use it as local context, then answer the current operator message.".to_string(),
        String::new(),
        "Recent messages:".to_string(),
    ];
    if entries.is_empty() {
        lines.push("(none)".to_string());
    } else {
        for entry in entries {
            lines.push(format!(
                "[{} / {}] {}",
                entry.source,
                entry.role,
                entry.text.trim()
            ));
        }
    }
    lines.extend([
        String::new(),
        "Current operator message:".to_string(),
        current_text.trim().to_string(),
    ]);
    Ok(lines.join("\n"))
}

fn expanded_limit(limit: usize) -> usize {
    limit.saturating_mul(8).max(limit)
}

fn visible_entries(entries: Vec<HistoryEntry>, limit: usize) -> Vec<HistoryEntry> {
    let visible = entries
        .into_iter()
        .filter(|entry| !is_control_plane_history(entry))
        .collect::<Vec<_>>();
    let start = visible.len().saturating_sub(limit);
    visible[start..].to_vec()
}

fn is_control_plane_history(entry: &HistoryEntry) -> bool {
    let text = entry.text.trim();
    if text.is_empty() {
        return true;
    }
    if entry.role == "operator" && text.starts_with('/') {
        return true;
    }
    if entry.role != "assistant" {
        return false;
    }
    text.starts_with("Started agent:\n")
        || text.starts_with("Failed to start agent:")
        || text.starts_with("qcold-status\t")
        || text.starts_with("agent-summary\t")
        || text.starts_with("Q-COLD connected repositories")
        || text.starts_with("Q-COLD Web control plane")
        || text.starts_with("/task creates Telegram task topics")
        || text.starts_with("/task can only be created")
        || text.starts_with("Recorded input for task ")
        || text.starts_with("Open Q-COLD Mini App.")
        || text.starts_with("Q-COLD Mini App URL is not configured.")
        || text.starts_with("Telegram user id:")
        || text.starts_with("Unknown Q-COLD command.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: &str, text: &str) -> HistoryEntry {
        HistoryEntry {
            id: 0,
            timestamp: 0,
            source: "web".to_string(),
            role: role.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn control_plane_history_excludes_agent_launch_noise() {
        assert!(is_control_plane_history(&entry(
            "operator",
            "/agent_start queue :: c1 exec 'large prompt'"
        )));
        assert!(is_control_plane_history(&entry(
            "assistant",
            "Started agent:\nagent\tqueue-1\tcmd=zellij ..."
        )));
        assert!(!is_control_plane_history(&entry(
            "operator",
            "what is the queue state?"
        )));
    }
}
