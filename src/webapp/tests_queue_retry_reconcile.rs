#[cfg(test)]
mod queue_retry_reconcile_tests {
    #![allow(clippy::unwrap_used)]

    use crate::test_support;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn failed_queue_row_with_live_recovery_agent_resumes_as_running() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("failed-live-recovery", "failed", 1);
        run.message = "closed:failed".to_string();
        let first = queue_item_fixture(&run.id, "first", 0, "success", Some("agent-1"));
        let mut second =
            queue_item_fixture(&run.id, "second", 1, "failed", Some("agent-2-r1"));
        second.message = "closed:failed".to_string();
        second.recovery_attempts = 1;
        let third = queue_item_fixture(&run.id, "third", 2, "pending", None);
        state::replace_web_queue(&run, &[first, second, third]).unwrap();
        state::insert_agent(&agent_fixture("agent-2-r1", &run.id)).unwrap();
        state::upsert_task_record(&state::new_task_record(
            "task/task-second".to_string(),
            "task-flow".to_string(),
            "second".to_string(),
            "prompt second".to_string(),
            "closed:failed".to_string(),
            None,
            None,
            Some("agent-2-r1".to_string()),
            None,
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(queue_run_needs_stale_reconcile(&run, &stored_items).unwrap());
        reconcile_stale_web_queue_run().unwrap();
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();

        assert_eq!(stored_run.status, "running");
        assert_eq!(
            stored_items
                .iter()
                .map(|item| (
                    item.id.as_str(),
                    item.status.as_str(),
                    item.agent_id.as_deref(),
                    item.message.as_str(),
                ))
                .collect::<Vec<_>>(),
            [
                ("first", "success", Some("agent-1"), ""),
                (
                    "second",
                    "running",
                    Some("agent-2-r1"),
                    "running recovery retry (agent-2-r1)",
                ),
                ("third", "pending", None, ""),
            ]
        );
    }

    fn queue_run_fixture(id: &str, status: &str, current_index: i64) -> state::QueueRunRow {
        state::QueueRunRow {
            id: id.to_string(),
            status: status.into(),
            execution_mode: "sequence".into(),
            execution_host: "local".into(),
            selected_agent_command: "c1".to_string(),
            remote_launcher: None,
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            selected_repo_root: None,
            selected_repo_name: None,
            track: queue_track(id),
            current_index,
            stop_requested: false,
            message: status.to_string(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn queue_item_fixture(
        run_id: &str,
        id: &str,
        position: i64,
        status: &str,
        agent_id: Option<&str>,
    ) -> state::QueueItemRow {
        state::QueueItemRow {
            id: id.to_string(),
            run_id: run_id.to_string(),
            position,
            depends_on: Vec::new(),
            prompt: format!("prompt {id}"),
            slug: format!("task-{id}"),
            repo_root: None,
            repo_name: None,
            execution_host: "local".into(),
            agent_command: "c1".to_string(),
            task_class: state::QueueTaskClass::Mid,
            remote_launcher: None,
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            agent_id: agent_id.map(str::to_string),
            status: status.into(),
            message: String::new(),
            attempts: 0,
            recovery_attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        }
    }

    fn agent_fixture(id: &str, run_id: &str) -> state::AgentRow {
        state::AgentRow {
            id: id.to_string(),
            track: queue_track(run_id),
            pid: std::process::id(),
            started_at: unix_now(),
            command: vec!["c1".to_string()],
            cwd: None,
            stdout_log_path: None,
            stderr_log_path: None,
        }
    }
}
