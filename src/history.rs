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

pub fn prompt_context(current_text: &str, limit: usize) -> Result<String> {
    let entries = load_recent(limit)?;
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
