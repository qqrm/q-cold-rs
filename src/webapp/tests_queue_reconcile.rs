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
        run.execution_mode = "graph".into();
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
    fn remote_native_missing_task_record_preserves_terminal_queue_row() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let mut run = queue_run_fixture("remote-native-terminal-record", "running", -1);
        run.selected_repo_root = Some(repo.display().to_string());
        let mut stale_item =
            queue_item_fixture(&run.id, "remote-native-terminal-record", 0, "running", None);
        stale_item.repo_root = Some(repo.display().to_string());
        stale_item.repo_name = Some("repo".to_string());
        stale_item.execution_host = "remote-native".into();
        stale_item.remote_launcher = Some("/bin/false".to_string());
        let agent_id = queue_agent_id(&stale_item);
        stale_item.agent_id = Some(agent_id.clone());
        let mut persisted_item = stale_item.clone();
        persisted_item.status = "success".into();
        persisted_item.message = "closed successfully remotely".to_string();
        state::replace_web_queue(&run, &[persisted_item]).unwrap();

        let outcome = missing_queue_task_record_outcome(
            &run.id,
            &stale_item,
            &agent_id,
            stale_item.attempts,
        )
        .unwrap();

        assert!(matches!(outcome, Some(QueueItemOutcome::Success)));
        let (_, items) = state::load_web_queue_run(&run.id).unwrap();
        assert_eq!(items[0].status, "success");
        assert_eq!(items[0].message, "closed successfully remotely");
    }

    #[test]
    fn remote_native_stale_update_preserves_terminal_queue_item() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("remote-native-terminal-stale-update", "running", -1);
        run.execution_mode = "graph".into();
        let mut item = queue_item_fixture(&run.id, "remote-stale", 0, "success", None);
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some("/bin/false".to_string());
        item.agent_id = Some(queue_agent_id(&item));
        item.message = "closed successfully remotely".to_string();
        let agent_id = item.agent_id.clone().unwrap();
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let outcome = update_queue_item_unless_terminal(
            &run.id,
            &item.id,
            "running",
            "waiting for remote-native task record visibility after remote-agent open",
            Some(&agent_id),
            item.attempts,
            None,
        )
        .unwrap();

        assert!(matches!(outcome, Some(QueueItemOutcome::Success)));
        let (_, items) = state::load_web_queue_run(&run.id).unwrap();
        assert_eq!(items[0].status, "success");
        assert_eq!(items[0].message, "closed successfully remotely");
    }

    #[test]
    fn failed_graph_queue_restarts_after_success_promotion_interruption() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-partial-reconcile", "failed", 1);
        run.execution_mode = "graph".into();
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

    #[test]
    fn failed_graph_queue_reconciles_remote_imported_success_for_legacy_local_item() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let mut run = queue_run_fixture("graph-remote-import", "failed", 0);
        run.execution_mode = "graph".into();
        let mut first = queue_item_fixture(&run.id, "first", 0, "failed", Some("qa-first"));
        first.repo_root = Some(repo.clone());
        first.message = "agent reached idle prompt after failed Q-COLD closeout".to_string();
        let mut second = queue_item_fixture(&run.id, "second", 1, "pending", None);
        second.repo_root = Some(repo.clone());
        second.depends_on = vec!["first".to_string()];
        state::replace_web_queue(&run, &[first, second]).unwrap();
        let metadata = format!(
            r#"{{"canonical_repo_root":"{repo}","remote_launcher":"remote-dev-env","remote_repo_root":"/remote/repo"}}"#
        );
        state::upsert_task_record(&state::new_task_record(
            "task/task-first".to_string(),
            "task-flow".to_string(),
            "first".to_string(),
            "prompt first".to_string(),
            "closed:success".to_string(),
            Some(repo.clone()),
            Some("/remote/repo".to_string()),
            Some("remote-user".to_string()),
            Some(metadata),
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
            panic!("expected remote-imported success record to restart queue");
        };

        assert_eq!(restarted_run.status, "running");
        assert_eq!(
            restarted_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [("first", "success"), ("second", "pending")]
        );
        assert_eq!(queue_ready_item_ids(&restarted_run, &restarted_items), ids(&["second"]));
    }

    #[test]
    fn queue_continue_restarts_failed_graph_after_stale_failed_row_resolves() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-continue-resolved", "failed", 0);
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

        handle_queue_continue_result(
            &HeaderMap::new(),
            &QueueContinueRequest {
                run_id: run.id.clone(),
            },
        )
        .unwrap();
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
    }

    #[test]
    fn queue_continue_accepts_already_resumed_graph_run() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-continue-race", "running", 0);
        run.execution_mode = "graph".into();
        let first = queue_item_fixture(&run.id, "first", 0, "success", Some("qa-first"));
        let mut second = queue_item_fixture(&run.id, "second", 1, "starting", Some("qa-second"));
        second.depends_on = vec!["first".to_string()];
        state::replace_web_queue(&run, &[first, second]).unwrap();

        handle_queue_continue_result(
            &HeaderMap::new(),
            &QueueContinueRequest {
                run_id: run.id.clone(),
            },
        )
        .unwrap();
        let (stored_run, _) = state::load_web_queue_run(&run.id).unwrap();

        assert_eq!(stored_run.unwrap().status, "running");
    }

    #[test]
    fn failed_graph_queue_restarts_after_newer_recovery_task_succeeds() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let mut run = queue_run_fixture("graph-recovery-record", "failed", 1);
        run.execution_mode = "graph".into();
        run.message = "closed:blocked".to_string();
        let first = queue_item_fixture(&run.id, "first", 0, "success", Some("agent-1"));
        let mut second = queue_item_fixture(&run.id, "second", 1, "failed", Some("agent-2"));
        second.repo_root = Some(repo.clone());
        second.message = "closed:blocked".to_string();
        let mut third = queue_item_fixture(&run.id, "third", 2, "pending", None);
        third.repo_root = Some(repo.clone());
        third.depends_on = vec!["first".to_string(), "second".to_string()];
        state::replace_web_queue(&run, &[first, second, third]).unwrap();

        let mut blocked = state::new_task_record(
            "task/task-second".to_string(),
            "task-flow".to_string(),
            "second".to_string(),
            "prompt second".to_string(),
            "closed:blocked".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/task-second".to_string()),
            Some("agent-2".to_string()),
            None,
        );
        blocked.updated_at = 100;
        state::upsert_task_record(&blocked).unwrap();
        let mut recovery = state::new_task_record(
            "task/task-second-recovery".to_string(),
            "task-flow".to_string(),
            "second recovery".to_string(),
            "prompt second recovery".to_string(),
            "closed:success".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/task-second-recovery".to_string()),
            Some("agent-recovery".to_string()),
            None,
        );
        recovery.updated_at = 200;
        state::upsert_task_record(&recovery).unwrap();

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

        assert_eq!(
            restarted_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [("first", "success"), ("second", "success"), ("third", "pending")]
        );
        assert_eq!(restarted_run.status, "running");
        assert_eq!(queue_ready_item_ids(&restarted_run, &restarted_items), ids(&["third"]));
    }

    #[test]
    fn failed_graph_queue_restarts_after_newer_reintegrate_task_succeeds() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let mut run = queue_run_fixture("graph-reintegrate-record", "failed", 1);
        run.execution_mode = "graph".into();
        run.message = "failed-closeout".to_string();
        let first = queue_item_fixture(&run.id, "EBSR2-00B", 0, "success", Some("agent-1"));
        let mut second = queue_item_fixture(&run.id, "EBSR2-00A", 1, "failed", Some("agent-2"));
        second.slug =
            "blockstore-ebs-v3f288-00a-fio-product-qemu-iouring-env-20260604-after-ebs00-p18320-20260605"
                .to_string();
        second.repo_root = Some(repo.clone());
        second.execution_host = "remote-native".into();
        second.remote_launcher = Some("remote-dev-env".to_string());
        second.message = "failed-closeout".to_string();
        second.started_at = 100;
        let mut third = queue_item_fixture(&run.id, "EBSR2-02", 2, "pending", None);
        third.repo_root = Some(repo.clone());
        third.depends_on = vec!["EBSR2-00A".to_string()];
        state::replace_web_queue(&run, &[first, second, third]).unwrap();

        let mut failed = state::new_task_record(
            "task/blockstore-ebs-v3f288-00a-fio-product-qemu-iouring-env-20260604-after-ebs00-p18320-20260605"
                .to_string(),
            "task-flow".to_string(),
            "original failed 00A".to_string(),
            "prompt failed 00A".to_string(),
            "failed-closeout".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/original-00a".to_string()),
            Some("agent-2".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        failed.updated_at = 110;
        state::upsert_task_record(&failed).unwrap();
        let mut reintegrated = state::new_task_record(
            "task/blockstore-ebs-v3f288-00a-fio-product-qemu-reintegrate-20260605".to_string(),
            "task-flow".to_string(),
            "00A reintegrated".to_string(),
            "prompt 00A reintegrated".to_string(),
            "closed:success".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/reintegrated-00a".to_string()),
            Some("agent-reintegrate".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        reintegrated.updated_at = 120;
        state::upsert_task_record(&reintegrated).unwrap();

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
        assert_eq!(
            restarted_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [
                ("EBSR2-00B", "success"),
                ("EBSR2-00A", "success"),
                ("EBSR2-02", "pending"),
            ]
        );
        assert_eq!(queue_ready_item_ids(&restarted_run, &restarted_items), ids(&["EBSR2-02"]));
    }

    #[test]
    fn failed_graph_queue_restarts_after_newer_relaunch_task_succeeds() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let mut run = queue_run_fixture("graph-relaunch-record", "failed", 0);
        run.execution_mode = "graph".into();
        run.message = "failed-closeout".to_string();
        let mut first = queue_item_fixture(&run.id, "EBSR2-05", 0, "failed", Some("agent-5"));
        first.slug = "blockstore-ebs-v3f288-05-original".to_string();
        first.repo_root = Some(repo.clone());
        first.execution_host = "remote-native".into();
        first.remote_launcher = Some("remote-dev-env".to_string());
        first.message = "failed-closeout".to_string();
        first.started_at = 100;
        let mut second = queue_item_fixture(&run.id, "EBSR2-06", 1, "pending", None);
        second.repo_root = Some(repo.clone());
        second.depends_on = vec!["EBSR2-05".to_string()];
        state::replace_web_queue(&run, &[first, second]).unwrap();

        let mut failed = state::new_task_record(
            "task/blockstore-ebs-v3f288-05-original".to_string(),
            "task-flow".to_string(),
            "original failed 05".to_string(),
            "prompt failed 05".to_string(),
            "failed-closeout".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/original-05".to_string()),
            Some("agent-5".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        failed.updated_at = 110;
        state::upsert_task_record(&failed).unwrap();
        let mut relaunched = state::new_task_record(
            "task/blockstore-ebs-v3f288-05-allocator-free-space-pressure-current-relaunch-20260605"
                .to_string(),
            "task-flow".to_string(),
            "05 relaunched".to_string(),
            "prompt 05 relaunched".to_string(),
            "closed:success".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/relaunch-05".to_string()),
            Some("agent-relaunch".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        relaunched.updated_at = 120;
        state::upsert_task_record(&relaunched).unwrap();

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
        assert_eq!(
            restarted_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [("EBSR2-05", "success"), ("EBSR2-06", "pending")]
        );
        assert_eq!(queue_ready_item_ids(&restarted_run, &restarted_items), ids(&["EBSR2-06"]));
    }

    #[test]
    fn failed_graph_queue_restarts_after_newer_repair_task_succeeds() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let mut run = queue_run_fixture("graph-repair-record", "failed", 0);
        run.execution_mode = "graph".into();
        run.message = "failed-closeout".to_string();
        let mut first = queue_item_fixture(&run.id, "EBSR2-05", 0, "failed", Some("agent-5"));
        first.slug =
            "blockstore-ebs-v3f288-05-allocator-free-space-pressure-parity-20260604-after-ebs00-p18326-20260605"
                .to_string();
        first.repo_root = Some(repo.clone());
        first.execution_host = "remote-native".into();
        first.remote_launcher = Some("remote-dev-env".to_string());
        first.message = "failed-closeout".to_string();
        first.started_at = 100;
        let mut second = queue_item_fixture(&run.id, "EBSR2-06", 1, "pending", None);
        second.repo_root = Some(repo.clone());
        second.depends_on = vec!["EBSR2-05".to_string()];
        state::replace_web_queue(&run, &[first, second]).unwrap();

        let mut failed = state::new_task_record(
            "task/blockstore-ebs-v3f288-05-allocator-free-space-pressure-parity-20260604-after-ebs00-p18326-20260605"
                .to_string(),
            "task-flow".to_string(),
            "original failed 05".to_string(),
            "prompt failed 05".to_string(),
            "failed-closeout".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/original-05".to_string()),
            Some("agent-5".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        failed.updated_at = 110;
        state::upsert_task_record(&failed).unwrap();
        let mut repair = state::new_task_record(
            "task/blockstore-ebs-v3f288-05-allocator-free-space-pressure-current-repair-20260605"
                .to_string(),
            "task-flow".to_string(),
            "05 repair".to_string(),
            "prompt 05 repair".to_string(),
            "closed:success".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/repair-05".to_string()),
            Some("agent-repair".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        repair.updated_at = 120;
        state::upsert_task_record(&repair).unwrap();

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
        assert_eq!(
            restarted_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [("EBSR2-05", "success"), ("EBSR2-06", "pending")]
        );
        assert_eq!(queue_ready_item_ids(&restarted_run, &restarted_items), ids(&["EBSR2-06"]));
    }

    #[test]
    fn failed_graph_queue_restarts_after_newer_numbered_repair_task_succeeds() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let mut run = queue_run_fixture("graph-numbered-repair-record", "failed", 0);
        run.execution_mode = "graph".into();
        run.message = "failed-closeout".to_string();
        let mut first = queue_item_fixture(&run.id, "EBSR2-05", 0, "failed", Some("agent-5"));
        first.slug =
            "blockstore-ebs-v3f288-05-allocator-free-space-pressure-parity-20260604-after-ebs00-p18326-20260605"
                .to_string();
        first.repo_root = Some(repo.clone());
        first.execution_host = "remote-native".into();
        first.remote_launcher = Some("remote-dev-env".to_string());
        first.message = "failed-closeout".to_string();
        first.started_at = 100;
        let mut second = queue_item_fixture(&run.id, "EBSR2-06", 1, "pending", None);
        second.repo_root = Some(repo.clone());
        second.depends_on = vec!["EBSR2-05".to_string()];
        state::replace_web_queue(&run, &[first, second]).unwrap();

        let mut failed = state::new_task_record(
            "task/blockstore-ebs-v3f288-05-allocator-free-space-pressure-parity-20260604-after-ebs00-p18326-20260605"
                .to_string(),
            "task-flow".to_string(),
            "original failed 05".to_string(),
            "prompt failed 05".to_string(),
            "failed-closeout".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/original-05".to_string()),
            Some("agent-5".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        failed.updated_at = 110;
        state::upsert_task_record(&failed).unwrap();
        let mut repair = state::new_task_record(
            "task/blockstore-ebs-v3f288-05-allocator-free-space-pressure-current-repair2-20260605"
                .to_string(),
            "task-flow".to_string(),
            "05 repair2".to_string(),
            "prompt 05 repair2".to_string(),
            "closed:success".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/repair2-05".to_string()),
            Some("agent-repair2".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        repair.updated_at = 120;
        state::upsert_task_record(&repair).unwrap();

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
        let statuses = restarted_items
            .iter()
            .map(|item| (item.id.as_str(), item.status.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(statuses, [("EBSR2-05", "success"), ("EBSR2-06", "pending")]);
        assert_eq!(queue_ready_item_ids(&restarted_run, &restarted_items), ids(&["EBSR2-06"]));
    }

    #[test]
    #[cfg(unix)]
    fn failed_graph_queue_restarts_while_remote_native_failed_closeout_retries() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let mut run = queue_run_fixture("graph-remote-native-retry", "failed", 1);
        run.execution_mode = "graph".into();
        run.message = "failed-closeout".to_string();
        let first = queue_item_fixture(&run.id, "first", 0, "success", Some("agent-1"));
        let mut second = queue_item_fixture(
            &run.id,
            "second",
            1,
            "failed",
            Some("qa-task-second"),
        );
        second.repo_root = Some(repo.clone());
        second.execution_host = "remote-native".into();
        second.remote_launcher = Some("/bin/true".to_string());
        second.message = "failed-closeout".to_string();
        let mut third = queue_item_fixture(&run.id, "third", 2, "pending", None);
        third.repo_root = Some(repo.clone());
        third.depends_on = vec!["second".to_string()];
        state::replace_web_queue(&run, &[first, second.clone(), third]).unwrap();
        let mut failed_closeout = state::new_task_record(
            "task/task-second".to_string(),
            "task-flow".to_string(),
            "second".to_string(),
            "prompt second".to_string(),
            "failed-closeout".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/task-second".to_string()),
            second.agent_id.clone(),
            None,
        );
        failed_closeout.updated_at = 100;
        state::upsert_task_record(&failed_closeout).unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(queue_run_needs_stale_reconcile(&run, &stored_items).unwrap());
        assert_eq!(queue_task_status(&second).unwrap(), None);
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let Some((restarted_run, restarted_items)) =
            restart_resolved_failed_queue_run(&stored_run.unwrap(), &stored_items).unwrap()
        else {
            panic!("expected live remote-native retry to restart the queue");
        };

        assert_eq!(restarted_run.status, "running");
        assert_eq!(
            restarted_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str(), item.message.as_str()))
                .collect::<Vec<_>>(),
            [
                ("first", "success", ""),
                (
                    "second",
                    "running",
                    "remote-native retry is still running after failed closeout",
                ),
                ("third", "pending", ""),
            ]
        );

        failed_closeout.status = "closed:success".to_string();
        failed_closeout.updated_at = 200;
        state::upsert_task_record(&failed_closeout).unwrap();
        let (_, retry_items) = state::load_web_queue_run(&restarted_run.id).unwrap();
        let retry_item = retry_items
            .iter()
            .find(|item| item.id == "second")
            .unwrap();
        let _ = remote_queue_sync_due(retry_item, "/bin/true", true);
        assert!(matches!(
            reconcile_queue_task_statuses(&restarted_run, &retry_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (resolved_run, resolved_items) = state::load_web_queue_run(&run.id).unwrap();
        let resolved_run = resolved_run.unwrap();

        assert_eq!(
            resolved_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [("first", "success"), ("second", "success"), ("third", "pending")]
        );
        assert_eq!(queue_ready_item_ids(&resolved_run, &resolved_items), ids(&["third"]));
    }

    #[test]
    #[cfg(unix)]
    fn stale_failed_graph_queue_resumes_live_remote_native_failed_closeout_retry() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let mut run = queue_run_fixture("stale-graph-remote-native-retry", "failed", 1);
        run.execution_mode = "graph".into();
        run.message = "failed-closeout".to_string();
        let first = queue_item_fixture(&run.id, "first", 0, "success", Some("agent-1"));
        let mut second = queue_item_fixture(
            &run.id,
            "second",
            1,
            "failed",
            Some("qa-task-second"),
        );
        second.repo_root = Some(repo.clone());
        second.execution_host = "remote-native".into();
        second.remote_launcher = Some("/bin/true".to_string());
        second.message = "failed-closeout".to_string();
        let mut third = queue_item_fixture(&run.id, "third", 2, "pending", None);
        third.repo_root = Some(repo.clone());
        third.depends_on = vec!["second".to_string()];
        state::replace_web_queue(&run, &[first, second.clone(), third]).unwrap();
        let mut failed_closeout = state::new_task_record(
            "task/task-second".to_string(),
            "task-flow".to_string(),
            "second".to_string(),
            "prompt second".to_string(),
            "failed-closeout".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/task-second".to_string()),
            second.agent_id.clone(),
            None,
        );
        failed_closeout.updated_at = 100;
        state::upsert_task_record(&failed_closeout).unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        resume_stale_active_queue_run(&run, stored_items).unwrap();
        let (resumed_run, resumed_items) = state::load_web_queue_run(&run.id).unwrap();
        let resumed_run = resumed_run.unwrap();

        assert_eq!(resumed_run.status, "running");
        assert_eq!(
            resumed_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str(), item.message.as_str()))
                .collect::<Vec<_>>(),
            [
                ("first", "success", ""),
                (
                    "second",
                    "running",
                    "remote-native retry is still running after failed closeout",
                ),
                ("third", "pending", ""),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn stale_running_remote_native_item_without_record_or_session_is_relaunched() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let mut run = queue_run_fixture("stale-remote-native-missing-record", "running", 1);
        run.execution_mode = "graph".into();
        let first = queue_item_fixture(&run.id, "first", 0, "success", Some("agent-1"));
        let mut second = queue_item_fixture(
            &run.id,
            "second",
            1,
            "running",
            Some("qa-task-second"),
        );
        second.repo_root = Some(repo.clone());
        second.execution_host = "remote-native".into();
        second.remote_launcher = Some("/bin/false".to_string());
        second.message = "waiting for remote-native task record visibility after remote-agent open"
            .to_string();
        let mut third = queue_item_fixture(&run.id, "third", 2, "pending", None);
        third.repo_root = Some(repo);
        third.depends_on = vec!["second".to_string()];
        state::replace_web_queue(&run, &[first, second, third]).unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        resume_stale_active_queue_run(&run, stored_items).unwrap();
        let (resumed_run, resumed_items) = state::load_web_queue_run(&run.id).unwrap();
        let resumed_run = resumed_run.unwrap();

        assert_eq!(resumed_run.status, "running");
        assert_eq!(
            resumed_items
                .iter()
                .map(|item| {
                    (
                        item.id.as_str(),
                        item.status.as_str(),
                        item.agent_id.as_deref(),
                        item.message.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            [
                ("first", "success", Some("agent-1"), ""),
                (
                    "second",
                    "pending",
                    None,
                    "remote-native task record and session are missing; relaunching item",
                ),
                ("third", "pending", None, ""),
            ]
        );
    }

    #[test]
    fn remote_native_running_item_skips_local_agent_failure_message() {
        let mut item = queue_item_fixture(
            "remote-native-run",
            "remote-item",
            0,
            "running",
            Some("qa-remote-item"),
        );
        item.execution_host = "remote-native".into();

        assert_eq!(
            queue_agent_failure_message(&item, "qa-remote-item"),
            None
        );
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
