#[cfg(test)]
mod terminal_metadata_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn requested_terminal_name_is_saved_as_display_metadata() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
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
                "1".to_string(),
                "c1".to_string(),
            ],
            cwd: None,
        };

        assign_terminal_display_name(&record, Some("atomic")).unwrap();

        let metadata = terminal_metadata_by_target().unwrap();
        assert_eq!(
            metadata
                .get("zellij:qcold-c1-1234:1")
                .and_then(|row| row.name.as_deref()),
            Some("atomic")
        );
    }
}
