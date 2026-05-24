#[cfg(test)]
mod zellij_name_tests {
    use super::*;

    #[test]
    fn zellij_layout_uses_requested_pane_name() {
        let layout = zellij_layout("client migration", "printf ok").unwrap();

        assert!(layout.contains("pane name=\"client migration\" command=\"sh\""));
        assert!(!layout.contains("pane name=\"c1-"));
    }

    #[test]
    fn terminal_title_sequence_uses_clean_short_name() {
        assert_eq!(
            terminal_title_sequence(" flow \x1b[31m ").as_deref(),
            Some("\x1b]0;flow [31m\x07")
        );
        assert!(terminal_title_sequence("\x1b\t").is_none());
        assert_eq!(
            terminal_title_shell_prefix(Some("flow")),
            "printf '\\033]0;%s\\007' 'flow'; "
        );
    }

    #[test]
    fn zellij_layout_title_prefix_avoids_control_escape_values() {
        let wrapped = format!("{}cc1", terminal_title_shell_prefix(Some("atomic")));
        let layout = zellij_layout("atomic", &wrapped).unwrap();

        assert!(layout.contains("033]0;%s"));
        assert!(!layout.contains("\\u001b"));
        assert!(!layout.contains("\\u0007"));
    }

    #[test]
    fn terminal_host_title_prefers_display_metadata() {
        let record = AgentRecord {
            id: "c1-1234".to_string(),
            track: "c1".to_string(),
            pid: std::process::id(),
            started_at: 456,
            command: vec![
                "zellij".to_string(),
                "--session".to_string(),
                "qcold-c1-1234".to_string(),
                "pane".to_string(),
                "terminal_0".to_string(),
                "cc1".to_string(),
            ],
            cwd: None,
        };
        let mut metadata = HashMap::new();
        metadata.insert(
            "zellij:qcold-c1-1234:terminal_0".to_string(),
            state::TerminalMetadataRow {
                target: "zellij:qcold-c1-1234:terminal_0".to_string(),
                name: Some("flow".to_string()),
                scope: None,
                updated_at: 123,
            },
        );

        assert_eq!(
            terminal_host_title(&record, &metadata).as_deref(),
            Some("flow")
        );
    }

    #[test]
    fn zellij_pane_name_is_compacted_and_limited() {
        assert_eq!(
            clean_zellij_pane_name(Some("  client   migration  ")).unwrap().as_deref(),
            Some("client migration")
        );
        assert_eq!(
            clean_zellij_pane_name(Some(&"x".repeat(90)))
                .unwrap()
                .unwrap()
                .chars()
                .count(),
            80
        );
        assert!(clean_zellij_pane_name(Some(" \t ")).is_err());
    }
}
