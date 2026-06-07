#[cfg(test)]
mod queue_status_sync_tests {
    #![allow(clippy::unwrap_used)]

    use crate::test_support;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn periodic_reconcile_restarts_failed_graph_after_stale_failed_row_resolves() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-periodic-resolved", "failed", 0);
        run.execution_mode = "graph".into();
        run.message = "failed-closeout".to_string();
        let mut first = queue_item_fixture(&run.id, "first", 0, "failed", Some("qa-first"));
        first.message = "failed-closeout".to_string();
        let mut second = queue_item_fixture(&run.id, "second", 1, "pending", None);
        second.depends_on = vec!["first".to_string()];
        state::replace_web_queue(&run, &[first, second]).unwrap();
        state::upsert_task_record(&state::new_task_record(
            "task/task-first".to_string(),
            "task-flow".to_string(),
            "first".to_string(),
            "prompt first".to_string(),
            "closed:success".to_string(),
            None,
            None,
            Some("qa-first".to_string()),
            None,
        ))
        .unwrap();

        reconcile_stale_web_queue_run().unwrap();
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();

        assert_eq!(stored_run.status, "running");
        assert_eq!(
            stored_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [("first", "success"), ("second", "pending")]
        );
        assert_eq!(queue_ready_item_ids(&stored_run, &stored_items), ids(&["second"]));
        assert!(test_web_queue_worker_spawned(&run.id));
    }

    #[test]
    fn queue_status_sync_interval_defaults_to_one_minute_and_allows_override() {
        let _guard = test_support::env_guard();

        assert_eq!(web_queue_status_sync_interval(), Duration::from_secs(60));

        std::env::set_var(WEB_QUEUE_STATUS_SYNC_INTERVAL_ENV, "7");
        assert_eq!(web_queue_status_sync_interval(), Duration::from_secs(7));

        std::env::set_var(WEB_QUEUE_STATUS_SYNC_INTERVAL_ENV, "0");
        assert_eq!(web_queue_status_sync_interval(), Duration::from_secs(60));
    }

    fn queue_ready_item_ids(
        run: &state::QueueRunRow,
        items: &[state::QueueItemRow],
    ) -> Vec<String> {
        queue_ready_items(run, items)
            .iter()
            .map(|item| item.id.clone())
            .collect()
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
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
            track: "queue-run".to_string(),
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
            task_class: "mid".into(),
            remote_launcher: None,
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            task_class: state::QueueTaskClass::Mid,
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
}
