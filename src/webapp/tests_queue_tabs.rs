#[cfg(test)]
mod queue_tabs_tests {
    use crate::test_support;

    use super::*;
    use rusqlite::Connection;
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
    fn empty_queue_tab_does_not_inherit_latest_run() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let default_run = queue_run_fixture("default-run", "running", -1);
        let default_item = queue_item_fixture("default-run", "default-item", 0, "pending", None);
        state::replace_web_queue(&default_run, &[default_item]).unwrap();
        state::create_web_queue_tab("client", "Client").unwrap();
        state::activate_web_queue_tab("client").unwrap();

        let (active_run, active_items) = state::load_web_queue().unwrap();
        let default_tab = state::load_web_queue_tab("default").unwrap().unwrap();
        let client_tab = state::load_web_queue_tab("client").unwrap().unwrap();

        assert!(active_run.is_none());
        assert!(active_items.is_empty());
        assert_eq!(default_tab.run_id.as_deref(), Some("default-run"));
        assert_eq!(client_tab.run_id, None);
    }

    #[test]
    fn queue_snapshot_hides_inactive_empty_tabs() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let default_run = queue_run_fixture("default-run", "running", -1);
        let default_item = queue_item_fixture("default-run", "default-item", 0, "pending", None);
        state::replace_web_queue(&default_run, &[default_item]).unwrap();
        state::create_web_queue_tab("empty", "Empty").unwrap();

        let snapshot = queue_snapshot();

        assert!(snapshot.tabs.iter().any(|tab| tab.id == "default"));
        assert!(!snapshot.tabs.iter().any(|tab| tab.id == "empty"));
    }

    #[test]
    fn queue_snapshot_keeps_active_empty_tab() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let default_run = queue_run_fixture("default-run", "running", -1);
        let default_item = queue_item_fixture("default-run", "default-item", 0, "pending", None);
        state::replace_web_queue(&default_run, &[default_item]).unwrap();
        state::create_web_queue_tab("empty", "Empty").unwrap();
        state::activate_web_queue_tab("empty").unwrap();

        let snapshot = queue_snapshot();

        let tab = snapshot
            .tabs
            .iter()
            .find(|tab| tab.id == "empty")
            .expect("active empty tab should be visible");
        assert!(tab.active);
        assert_eq!(tab.count, 0);
    }

    #[test]
    fn creating_queue_tab_activates_it() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let response = handle_queue_tab_create(
            &HeaderMap::new(),
            &QueueTabCreateRequest {
                label: Some("Client".to_string()),
            },
        );

        assert!(response.ok, "{}", response.output);
        let tab_id = response.output.split('\t').nth(1).unwrap();
        let tab = state::load_web_queue_tab(tab_id).unwrap().unwrap();
        assert!(tab.active);
    }

    #[test]
    fn queue_tab_mutation_invalidates_stale_dashboard_cache() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        invalidate_dashboard_state_cache();
        let default_run = queue_run_fixture("default-run", "failed", -1);
        let default_item = queue_item_fixture("default-run", "default-item", 0, "failed", None);
        state::replace_web_queue(&default_run, &[default_item]).unwrap();
        refresh_dashboard_state_cache();

        state::create_and_activate_web_queue_tab("client", "Client").unwrap();
        let refresh_lock = DASHBOARD_STATE_REFRESHING.get_or_init(|| Mutex::new(false));
        *refresh_lock.lock().unwrap() = true;
        refresh_dashboard_state_after_mutation(true);
        *refresh_lock.lock().unwrap() = false;

        let json = cached_dashboard_state_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let queue = &parsed["queue"];
        assert_eq!(queue["active_tab_id"].as_str(), Some("client"));
        let tabs = queue["tabs"].as_array().unwrap();
        let client = tabs
            .iter()
            .find(|tab| tab["id"].as_str() == Some("client"))
            .expect("fresh state should include active empty tab");
        assert_eq!(client["count"].as_u64(), Some(0));
        assert_eq!(client["active"].as_bool(), Some(true));
        invalidate_dashboard_state_cache();
    }

    #[test]
    fn queue_run_can_target_tab_without_switching_backend_active_tab() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let default_run = queue_run_fixture("default-run", "running", -1);
        let default_item = queue_item_fixture("default-run", "default-item", 0, "pending", None);
        state::replace_web_queue(&default_run, &[default_item]).unwrap();
        state::create_web_queue_tab("client", "Client").unwrap();
        let client_run = queue_run_fixture("client-run", "waiting", -1);
        let client_item = queue_item_fixture("client-run", "client-item", 0, "pending", None);

        state::replace_web_queue_for_tab("client", &client_run, &[client_item]).unwrap();

        let (active_run, active_items) = state::load_web_queue().unwrap();
        let default_tab = state::load_web_queue_tab("default").unwrap().unwrap();
        let client_tab = state::load_web_queue_tab("client").unwrap().unwrap();
        assert_eq!(active_run.unwrap().id, "default-run");
        assert_eq!(active_items[0].id, "default-item");
        assert!(default_tab.active);
        assert!(!client_tab.active);
        assert_eq!(default_tab.run_id.as_deref(), Some("default-run"));
        assert_eq!(client_tab.run_id.as_deref(), Some("client-run"));
    }

    #[test]
    fn queue_stop_can_target_visible_tab_without_backend_active_tab() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let default_run = queue_run_fixture("default-run", "running", -1);
        let default_item = queue_item_fixture("default-run", "default-item", 0, "pending", None);
        state::replace_web_queue(&default_run, &[default_item]).unwrap();
        state::create_web_queue_tab("client", "Client").unwrap();
        let client_run = queue_run_fixture("client-run", "running", -1);
        let client_item = queue_item_fixture("client-run", "client-item", 0, "pending", None);
        state::replace_web_queue_for_tab("client", &client_run, &[client_item]).unwrap();

        let response = handle_queue_stop(
            &HeaderMap::new(),
            &QueueStopRequest {
                run_id: Some("client-run".to_string()),
            },
        );

        let (default_run, _) = state::load_web_queue_run("default-run").unwrap();
        let (client_run, _) = state::load_web_queue_run("client-run").unwrap();
        assert!(response.ok);
        assert_eq!(default_run.unwrap().status, "running");
        assert_eq!(client_run.unwrap().status, "stopping");
    }

    #[test]
    fn queue_tab_snapshot_carries_records_for_each_tab() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let default_run = queue_run_fixture("default-run", "running", -1);
        let default_item = queue_item_fixture("default-run", "default-item", 0, "pending", None);
        state::replace_web_queue(&default_run, &[default_item]).unwrap();
        state::create_web_queue_tab("client", "Client").unwrap();
        let client_run = queue_run_fixture("client-run", "waiting", -1);
        let client_item = queue_item_fixture("client-run", "client-item", 0, "pending", None);
        state::replace_web_queue_for_tab("client", &client_run, &[client_item]).unwrap();

        let snapshot = queue_snapshot();
        let default_tab = snapshot
            .tabs
            .iter()
            .find(|tab| tab.id == "default")
            .expect("default tab should be present");
        let client_tab = snapshot
            .tabs
            .iter()
            .find(|tab| tab.id == "client")
            .expect("client tab should be present");

        assert_eq!(default_tab.run.as_ref().unwrap().id, "default-run");
        assert_eq!(default_tab.records[0].id, "default-item");
        assert_eq!(client_tab.run.as_ref().unwrap().id, "client-run");
        assert_eq!(client_tab.records[0].id, "client-item");
    }

    #[test]
    fn duplicate_queue_run_tab_reference_is_repaired() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let default_run = queue_run_fixture("default-run", "running", -1);
        let default_item = queue_item_fixture("default-run", "default-item", 0, "pending", None);
        state::replace_web_queue(&default_run, &[default_item]).unwrap();
        state::create_web_queue_tab("client", "Client").unwrap();
        let db = Connection::open(temp.path().join("qcold.sqlite3")).unwrap();
        db.execute(
            "update web_queue_tabs set run_id = 'default-run' where id = 'client'",
            [],
        )
        .unwrap();

        let snapshot = queue_snapshot();
        let default_tab = snapshot
            .tabs
            .iter()
            .find(|tab| tab.id == "default")
            .expect("default tab should be present");
        let stored_client_tab = state::load_web_queue_tab("client").unwrap().unwrap();

        assert_eq!(default_tab.run_id.as_deref(), Some("default-run"));
        assert_eq!(default_tab.count, 1);
        assert!(!snapshot.tabs.iter().any(|tab| tab.id == "client"));
        assert_eq!(stored_client_tab.run_id, None);
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
    fn queue_live_work_uses_precomputed_running_agent_ids() {
        let run = queue_run_fixture("client-run", "failed", -1);
        let item = queue_item_fixture("client-run", "client-item", 0, "success", Some("agent-1"));
        let mut running_agents = HashSet::new();

        assert!(!queue_run_has_live_work_with_agents(
            &run,
            std::slice::from_ref(&item),
            &running_agents
        ));

        running_agents.insert("agent-1".to_string());

        assert!(queue_run_has_live_work_with_agents(
            &run,
            &[item],
            &running_agents
        ));
    }

    #[test]
    fn queue_tab_delete_removes_stopped_run_rows() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        state::create_web_queue_tab("client", "Client").unwrap();
        state::activate_web_queue_tab("client").unwrap();
        let run = queue_run_fixture("client-run", "stopped", -1);
        let first = queue_item_fixture("client-run", "first", 0, "success", None);
        let second = queue_item_fixture("client-run", "second", 1, "failed", None);
        state::replace_web_queue(&run, &[first, second]).unwrap();

        let response = handle_queue_tab_delete(
            &HeaderMap::new(),
            &QueueTabRequest {
                tab_id: "client".to_string(),
            },
        );

        assert!(response.ok, "{}", response.output);
        assert!(state::load_web_queue_tab("client").unwrap().is_none());
        assert!(state::load_web_queue_run("client-run").unwrap().0.is_none());
        assert!(state::load_web_queue_items().unwrap().is_empty());
    }

    #[test]
    fn queue_tab_delete_falls_back_to_non_empty_tab() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        state::load_web_queue_tabs().unwrap();
        state::create_web_queue_tab("client", "Client").unwrap();
        let run = queue_run_fixture("client-run", "stopped", -1);
        let item = queue_item_fixture("client-run", "client-item", 0, "success", None);
        state::replace_web_queue_for_tab("client", &run, &[item]).unwrap();
        state::create_web_queue_tab("draft", "Draft").unwrap();
        state::activate_web_queue_tab("draft").unwrap();

        let response = handle_queue_tab_delete(
            &HeaderMap::new(),
            &QueueTabRequest {
                tab_id: "draft".to_string(),
            },
        );

        assert!(response.ok, "{}", response.output);
        assert!(state::load_web_queue_tab("draft").unwrap().is_none());
        assert!(state::load_web_queue_tab("client").unwrap().unwrap().active);
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

    #[test]
    fn queue_snapshot_does_not_block_on_remote_native_sync() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let mut run = queue_run_fixture("remote-run", "running", -1);
        run.execution_mode = "graph".into();
        run.execution_host = "remote-native".into();
        run.selected_repo_root = Some(repo.display().to_string());
        let mut item =
            queue_item_fixture("remote-run", "remote-item", 0, "running", Some("agent-remote"));
        item.execution_host = "remote-native".into();
        item.repo_root = Some(repo.display().to_string());
        item.remote_launcher = Some("/bin/false".to_string());
        state::replace_web_queue(&run, &[item]).unwrap();

        let snapshot = queue_snapshot();

        assert_eq!(snapshot.count, 1);
        assert_eq!(snapshot.records[0].id, "remote-item");
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.tabs[0].count, 1);
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
}
