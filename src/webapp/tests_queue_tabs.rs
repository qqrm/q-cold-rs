#[cfg(test)]
mod queue_tabs_tests {
    use crate::test_support;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn queue_tabs_keep_runs_isolated_and_switch_active_run() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let default_run = queue_run_fixture("default-run", "running", -1);
        let default_item = queue_item_fixture("default-run", "default-item", 0, "pending", None);
        state::replace_web_queue(&default_run, &[default_item]).unwrap();
        state::create_web_queue_tab("client", "Client").unwrap();
        state::activate_web_queue_tab("client").unwrap();
        let client_run = queue_run_fixture("client-run", "waiting", -1);
        let client_item = queue_item_fixture("client-run", "client-item", 0, "pending", None);
        state::replace_web_queue(&client_run, &[client_item]).unwrap();

        let (active_run, active_items) = state::load_web_queue().unwrap();
        assert_eq!(active_run.unwrap().id, "client-run");
        assert_eq!(active_items[0].id, "client-item");
        state::activate_web_queue_tab("default").unwrap();
        let (default_active_run, default_active_items) = state::load_web_queue().unwrap();

        assert_eq!(default_active_run.unwrap().id, "default-run");
        assert_eq!(default_active_items[0].id, "default-item");
        assert_eq!(state::load_web_queue_runs().unwrap().len(), 2);
    }

    #[test]
    fn queue_tab_replacement_prunes_superseded_run() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let old_run = queue_run_fixture("old-run", "running", -1);
        let old_item = queue_item_fixture("old-run", "old-item", 0, "pending", None);
        state::replace_web_queue(&old_run, &[old_item]).unwrap();
        let new_run = queue_run_fixture("new-run", "running", -1);
        let new_item = queue_item_fixture("new-run", "new-item", 0, "pending", None);

        state::replace_web_queue(&new_run, &[new_item]).unwrap();

        let runs = state::load_web_queue_runs().unwrap();
        let items = state::load_web_queue_items().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0.id, "new-run");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "new-item");
    }

    #[test]
    fn queue_tab_delete_rejects_running_work() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        state::create_web_queue_tab("client", "Client").unwrap();
        state::activate_web_queue_tab("client").unwrap();
        let run = queue_run_fixture("client-run", "running", -1);
        let item = queue_item_fixture("client-run", "client-item", 0, "running", Some("agent-1"));
        state::replace_web_queue(&run, &[item]).unwrap();

        let response = handle_queue_tab_delete(
            &HeaderMap::new(),
            &QueueTabRequest {
                tab_id: "client".to_string(),
            },
        );

        assert!(!response.ok);
        assert!(response.output.contains("running work"));
        assert!(state::load_web_queue_tab("client").unwrap().is_some());
    }

    #[test]
    fn queue_tab_snapshot_marks_live_items_running() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        state::create_web_queue_tab("client", "Client").unwrap();
        state::activate_web_queue_tab("client").unwrap();
        let run = queue_run_fixture("client-run", "failed", -1);
        let item = queue_item_fixture("client-run", "client-item", 0, "running", None);
        state::replace_web_queue(&run, &[item]).unwrap();

        let snapshot = queue_snapshot();
        let tab = snapshot
            .tabs
            .iter()
            .find(|tab| tab.id == "client")
            .expect("client tab should be present");

        assert!(tab.running);
    }

    fn queue_run_fixture(id: &str, status: &str, current_index: i64) -> state::QueueRunRow {
        state::QueueRunRow {
            id: id.to_string(),
            status: status.to_string(),
            execution_mode: "sequence".to_string(),
            selected_agent_command: "c1".to_string(),
            remote_launcher: None,
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
            remote_launcher: None,
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
