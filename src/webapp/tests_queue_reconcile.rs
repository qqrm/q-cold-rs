#[cfg(test)]
mod queue_reconcile_tests {
    #![allow(clippy::unwrap_used)]

    use crate::test_support;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn failed_graph_queue_restarts_after_blocked_prerequisite_later_succeeds() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-resolved-blocker", "failed", 1);
        run.execution_mode = "graph".to_string();
        run.message = "closed:blocked".to_string();
        let first = queue_item_fixture(&run.id, "first", 0, "success", Some("agent-1"));
        let mut second = queue_item_fixture(&run.id, "second", 1, "failed", Some("agent-2"));
        second.message = "closed:blocked".to_string();
        let mut third = queue_item_fixture(&run.id, "third", 2, "pending", None);
        third.depends_on = vec!["first".to_string(), "second".to_string()];
        let mut fourth = queue_item_fixture(&run.id, "fourth", 3, "pending", None);
        fourth.depends_on = vec!["third".to_string()];
        state::replace_web_queue(&run, &[first, second, third, fourth]).unwrap();
        state::upsert_task_record(&state::new_task_record(
            "task/task-second".to_string(),
            "task-flow".to_string(),
            "second".to_string(),
            "prompt second".to_string(),
            "closed:success".to_string(),
            None,
            None,
            Some("agent-2".to_string()),
            None,
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(queue_run_needs_stale_reconcile(&run, &stored_items).unwrap());
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let Some((restarted_run, restarted_items)) =
            restart_resolved_failed_queue_run(&stored_run.unwrap(), &stored_items).unwrap()
        else {
            panic!("expected resolved failed queue to restart");
        };

        assert_eq!(restarted_run.status, "running");
        assert_eq!(restarted_run.message, "resuming after resolved blocked task");
        assert_eq!(
            restarted_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [
                ("first", "success"),
                ("second", "success"),
                ("third", "pending"),
                ("fourth", "pending"),
            ]
        );
        assert_eq!(queue_ready_item_ids(&restarted_run, &restarted_items), ids(&["third"]));
    }

    #[test]
    fn failed_graph_queue_restarts_after_success_promotion_interruption() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-partial-reconcile", "failed", 1);
        run.execution_mode = "graph".to_string();
        let first = queue_item_fixture(&run.id, "first", 0, "success", Some("agent-1"));
        let second = queue_item_fixture(&run.id, "second", 1, "success", Some("agent-2"));
        let mut third = queue_item_fixture(&run.id, "third", 2, "pending", None);
        third.depends_on = vec!["first".to_string(), "second".to_string()];
        state::replace_web_queue(&run, &[first, second, third]).unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(queue_run_needs_stale_reconcile(&run, &stored_items).unwrap());
        let Some((restarted_run, restarted_items)) =
            restart_resolved_failed_queue_run(&run, &stored_items).unwrap()
        else {
            panic!("expected partially reconciled queue to restart");
        };

        assert_eq!(restarted_run.status, "running");
        assert_eq!(queue_ready_item_ids(&restarted_run, &restarted_items), ids(&["third"]));
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
            status: status.to_string(),
            execution_mode: "sequence".to_string(),
            selected_agent_command: "c1".to_string(),
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
            agent_command: "c1".to_string(),
            agent_id: agent_id.map(str::to_string),
            status: status.to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        }
    }
}
