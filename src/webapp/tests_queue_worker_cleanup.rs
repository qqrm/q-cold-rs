#[cfg(test)]
mod queue_worker_cleanup_tests {
    use super::*;

    #[test]
    fn queue_agent_cleanup_deletes_stale_record_after_terminal_cleanup_failure() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let agent = state::AgentRow {
            id: "qa-stale-cleanup".to_string(),
            track: "queue-test".to_string(),
            pid: u32::MAX,
            started_at: unix_now(),
            command: vec![
                "tmux".to_string(),
                "new-session".to_string(),
                "-s".to_string(),
                "qcold-qa-stale-cleanup".to_string(),
                "c1".to_string(),
            ],
            cwd: None,
            stdout_log_path: None,
            stderr_log_path: None,
        };
        state::insert_agent(&agent).unwrap();

        let message = cleanup_queue_agent(&agent.id);
        let agents = state::load_agents(&temp.path().join("legacy-agents.tsv")).unwrap();

        assert!(message.contains("agent record deleted"));
        assert!(!agents.iter().any(|row| row.id == agent.id));
    }
}
