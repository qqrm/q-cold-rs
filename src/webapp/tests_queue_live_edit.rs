#[cfg(test)]
mod queue_live_edit_tests {
    use crate::test_support;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn running_graph_queue_updates_pending_item_plan() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-update", "running", -1);
        run.execution_mode = "graph".to_string();
        let upstream =
            queue_item_fixture("graph-update", "upstream", 0, "running", Some("agent-1"));
        let pending = queue_item_fixture("graph-update", "pending", 1, "waiting", None);
        state::replace_web_queue(&run, &[upstream, pending]).unwrap();

        let response = handle_queue_update(
            &HeaderMap::new(),
            QueueUpdateRequest {
                run_id: run.id.clone(),
                items: vec![QueueUpdateItemRequest {
                    id: "pending".to_string(),
                    prompt: "updated prompt".to_string(),
                    position: Some(2),
                    depends_on: Some(vec!["upstream".to_string(), "missing".to_string()]),
                    repo_root: None,
                    repo_name: None,
                    agent_command: None,
                }],
            },
        );

        assert!(response.ok, "{}", response.output);
        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let pending = stored_items
            .iter()
            .find(|item| item.id == "pending")
            .unwrap();
        assert_eq!(pending.prompt, "updated prompt");
        assert_eq!(pending.position, 2);
        assert_eq!(pending.depends_on, vec!["upstream".to_string()]);
    }

    #[test]
    fn running_graph_queue_append_preserves_future_wave_dependencies() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-append", "running", -1);
        run.execution_mode = "graph".to_string();
        let current_wave =
            queue_item_fixture("graph-append", "current-wave", 0, "running", Some("agent-1"));
        state::replace_web_queue(&run, &[current_wave]).unwrap();

        let response = handle_queue_append(
            &HeaderMap::new(),
            QueueAppendRequest {
                run_id: run.id.clone(),
                items: vec![QueueRunItemRequest {
                    id: Some("later-wave".to_string()),
                    prompt: "future wave prompt".to_string(),
                    slug: None,
                    depends_on: Some(vec!["current-wave".to_string(), "missing".to_string()]),
                    repo_root: None,
                    repo_name: None,
                    agent_command: None,
                }],
            },
        );

        assert!(response.ok, "{}", response.output);
        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let appended = stored_items
            .iter()
            .find(|item| item.id == "later-wave")
            .unwrap();
        assert_eq!(appended.depends_on, vec!["current-wave".to_string()]);
    }

    #[test]
    fn running_graph_queue_rejects_active_item_plan_update() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-update-active", "running", -1);
        run.execution_mode = "graph".to_string();
        let active =
            queue_item_fixture("graph-update-active", "active", 0, "running", Some("agent-1"));
        state::replace_web_queue(&run, &[active]).unwrap();

        let response = handle_queue_update(
            &HeaderMap::new(),
            QueueUpdateRequest {
                run_id: run.id.clone(),
                items: vec![QueueUpdateItemRequest {
                    id: "active".to_string(),
                    prompt: "updated prompt".to_string(),
                    position: Some(1),
                    depends_on: Some(Vec::new()),
                    repo_root: None,
                    repo_name: None,
                    agent_command: None,
                }],
            },
        );

        assert!(!response.ok);
        assert!(response.output.contains("already active"));
    }

    #[test]
    fn waiting_sequence_queue_updates_future_item_plan() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("sequence-update", "waiting", 0);
        let active = queue_item_fixture("sequence-update", "active", 0, "running", Some("agent-1"));
        let pending = queue_item_fixture("sequence-update", "pending", 1, "pending", None);
        state::replace_web_queue(&run, &[active, pending]).unwrap();

        let response = handle_queue_update(
            &HeaderMap::new(),
            QueueUpdateRequest {
                run_id: run.id.clone(),
                items: vec![QueueUpdateItemRequest {
                    id: "pending".to_string(),
                    prompt: "sequence prompt".to_string(),
                    position: Some(2),
                    depends_on: Some(vec!["active".to_string()]),
                    repo_root: None,
                    repo_name: None,
                    agent_command: None,
                }],
            },
        );

        assert!(response.ok, "{}", response.output);
        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let pending = stored_items
            .iter()
            .find(|item| item.id == "pending")
            .unwrap();
        assert_eq!(pending.prompt, "sequence prompt");
        assert_eq!(pending.position, 2);
        assert!(pending.depends_on.is_empty());
    }

    #[test]
    fn running_queue_rejects_future_item_move_before_active_cursor() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("sequence-cursor", "running", 1);
        let active = queue_item_fixture("sequence-cursor", "active", 1, "running", Some("agent-1"));
        let pending = queue_item_fixture("sequence-cursor", "pending", 2, "pending", None);
        state::replace_web_queue(&run, &[active, pending]).unwrap();

        let response = handle_queue_update(
            &HeaderMap::new(),
            QueueUpdateRequest {
                run_id: run.id.clone(),
                items: vec![QueueUpdateItemRequest {
                    id: "pending".to_string(),
                    prompt: "updated prompt".to_string(),
                    position: Some(1),
                    depends_on: Some(Vec::new()),
                    repo_root: None,
                    repo_name: None,
                    agent_command: None,
                }],
            },
        );

        assert!(!response.ok);
        assert!(response.output.contains("active cursor"));
    }

    #[test]
    fn running_queue_removes_future_pending_item() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("remove-future", "running", 0);
        let active = queue_item_fixture("remove-future", "active", 0, "running", Some("agent-1"));
        let pending = queue_item_fixture("remove-future", "pending", 1, "pending", None);
        state::replace_web_queue(&run, &[active, pending]).unwrap();

        let response = handle_queue_remove(
            &HeaderMap::new(),
            &QueueRemoveRequest {
                run_id: run.id.clone(),
                item_id: "pending".to_string(),
                task_id: None,
                agent_id: None,
            },
        );

        assert!(response.ok, "{}", response.output);
        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert_eq!(stored_items.len(), 1);
        assert_eq!(stored_items[0].id, "active");
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
