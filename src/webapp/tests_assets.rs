#[cfg(test)]
mod asset_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn web_terminal_slash_menu_uses_codex_command_prefixes() {
        assert!(APP_JS.contains("['model', 'choose what model and reasoning effort to use']"));
        assert!(APP_JS.contains("['quit', 'exit Codex', true]"));
        assert!(APP_JS.contains("input.value = `/${match[0]}`;"));
        assert!(APP_JS.contains("function terminalSlashCommandMatches(query)"));
        assert!(APP_JS.contains("else if (name.startsWith(needle)) prefix.push(command);"));
        assert!(!APP_JS.contains("['/q'"));
        assert!(!APP_JS.contains("Help menu"));
    }
}
