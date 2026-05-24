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

    #[test]
    fn running_named_terminal_record_matches_live_track_name() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path());
        state::insert_agent(&state::AgentRow {
            id: "c1-atomic".to_string(),
            track: "c1".to_string(),
            pid: std::process::id(),
            started_at: 456,
            command: zellij_agent_command("qcold-c1-atomic"),
            cwd: None,
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();
        state::save_terminal_metadata("zellij:qcold-c1-atomic:1", Some("Atomic"), None).unwrap();

        let record = running_named_terminal_record("c1", "atomic").unwrap().unwrap();

        assert_eq!(record.id, "c1-atomic");
        assert!(running_named_terminal_record("c2", "atomic").unwrap().is_none());
    }

    #[test]
    fn running_named_terminal_record_ignores_exited_name() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path());
        state::insert_agent(&state::AgentRow {
            id: "c1-stale".to_string(),
            track: "c1".to_string(),
            pid: u32::MAX,
            started_at: 456,
            command: zellij_agent_command("qcold-c1-stale"),
            cwd: None,
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();
        state::save_terminal_metadata("zellij:qcold-c1-stale:1", Some("Atomic"), None).unwrap();

        assert!(running_named_terminal_record("c1", "atomic").unwrap().is_none());
    }

    fn zellij_agent_command(session: &str) -> Vec<String> {
        vec![
            "zellij".to_string(),
            "--session".to_string(),
            session.to_string(),
            "pane".to_string(),
            "1".to_string(),
            "cc1".to_string(),
        ]
    }
}
