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

    #[test]
    fn graph_queue_active_run_keeps_wave_append_controls() {
        assert!(APP_JS.contains("addWaveButton.disabled = !queueGraphAppendable();"));
        assert!(APP_JS.contains("function queueBackendRunAppendable()"));
        assert!(APP_JS.contains("items: [{ id: item.id, prompt, depends_on: dependsOn }]"));
        assert!(APP_JS.contains(
            "queueWaves = normalizeQueueWaves(preservedWaves, queueItems, { pruneBackendEmpty: true });"
        ));
    }

    #[test]
    fn graph_queue_active_run_prunes_empty_non_final_waves() {
        assert!(APP_JS.contains("function pruneEmptyBackendQueueWaves(waves, items)"));
        assert!(APP_JS.contains("{ pruneBackendEmpty: true }"));
        assert!(APP_JS.contains("wavesWithItems.has(wave.id) || index === waves.length - 1"));
    }
}
