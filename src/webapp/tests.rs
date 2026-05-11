#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use crate::test_support;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn daemon_paths_are_scoped_by_listen_address() {
        let temp = tempdir().unwrap();
        let paths = WebappDaemonPaths::from_state_dir(temp.path(), "192.0.2.10:8787");
        assert_eq!(
            paths.pid,
            temp.path().join("webapp-192-0-2-10-8787.pid")
        );
        assert_eq!(
            paths.stdout_log,
            temp.path()
                .join("logs")
                .join("webapp-192-0-2-10-8787.out.log")
        );
        assert_eq!(
            paths.stderr_log,
            temp.path()
                .join("logs")
                .join("webapp-192-0-2-10-8787.err.log")
        );
    }

    #[test]
    fn empty_daemon_path_id_falls_back_to_default() {
        assert_eq!(sanitize_daemon_id(":::////"), "default");
    }

    #[test]
    fn host_agent_classifier_detects_console_codex() {
        let args = vec![
            "/opt/qcold-demo/bin/codex".to_string(),
            "exec".to_string(),
            "inspect".to_string(),
        ];
        assert_eq!(classify_host_agent(&args).as_deref(), Some("codex"));
    }

    #[test]
    fn host_agent_classifier_ignores_codex_node_wrapper() {
        let args = vec![
            "node".to_string(),
            "/opt/qcold-demo/bin/codex".to_string(),
            "exec".to_string(),
        ];
        assert_eq!(classify_host_agent(&args), None);
    }

    #[test]
    fn host_agent_classifier_ignores_xtask_taskflow_processes() {
        let args = vec![
            "/tmp/repository-taskflow/example/debug/xtask".to_string(),
            "task".to_string(),
            "enter".to_string(),
        ];
        assert_eq!(classify_host_agent(&args), None);
    }

    #[test]
    fn host_agent_classifier_detects_qcold_web_daemon() {
        let args = vec![
            "/opt/qcold-demo/bin/qcold".to_string(),
            "telegram".to_string(),
            "serve".to_string(),
            "--listen".to_string(),
            "127.0.0.1:8787".to_string(),
            "--daemon-child".to_string(),
        ];
        assert_eq!(classify_host_agent(&args).as_deref(), Some("web-daemon"));
    }

    #[test]
    fn terminal_key_mapping_supports_history_navigation() {
        let key = clean_terminal_key("ArrowUp").unwrap();

        assert_eq!(key, TerminalKey::Up);
        assert_eq!(key.tmux(), "Up");
        assert_eq!(key.zellij(), "Up");
        assert!(clean_terminal_key("$(touch /tmp/nope)").is_err());
    }

    #[test]
    fn terminal_send_request_supports_literal_slash_commands() {
        let request = TerminalSendRequest {
            target: "main:0.1".to_string(),
            text: Some("/new".to_string()),
            mode: Some("literal".to_string()),
            key: None,
            submit: Some(true),
        };

        match terminal_input_from_request(&request).unwrap() {
            TerminalInput::Literal { text, submit } => {
                assert_eq!(text, "/new");
                assert!(submit);
            }
            _ => panic!("expected literal input"),
        }
    }

    #[test]
    fn agent_start_template_keeps_agent_workspace_host_side() {
        let template = agent_start_template("/workspace/repo");
        assert!(template.contains("/agent_start --cwd '/workspace/repo' <track>"));
        assert!(template.contains("host-side agent workspace"));
        assert!(template.contains("do not enter a devcontainer from $QCOLD_AGENT_WORKTREE"));
        assert!(template.contains("enter that managed task worktree and its devcontainer"));
    }

    #[test]
    fn queue_task_instruction_starts_managed_task() {
        let item = state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "do focused work".to_string(),
            slug: "task-run-01".to_string(),
            repo_root: Some("/workspace/repo".to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };

        let instruction = queue_task_instruction(&item);
        assert!(instruction.contains("Q-COLD_TASK_PACKET"));
        assert!(instruction.contains("repo_root: /workspace/repo"));
        assert!(instruction.contains("task_slug: task-run-01"));
        assert!(instruction.contains("selected_command: c1"));
        assert!(instruction.contains("do not run cargo qcold task open"));
        assert!(instruction.contains("task_env: .task/task.env"));
        assert!(instruction.contains("task_logs: .task/logs/"));
        assert!(instruction.contains("pause_or_blocked_only_for: business decision"));
        assert!(instruction.contains("operator_request: |\n  do focused work"));
        assert!(!instruction.contains("home base for /workspace/repo"));
    }

    #[test]
    fn queue_terminal_scope_uses_managed_task_slug() {
        let item = state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "do focused work".to_string(),
            slug: "task-mozgpaqk-03".to_string(),
            repo_root: Some("/workspace/repo".to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };

        assert_eq!(queue_terminal_scope(&item), "task/task-mozgpaqk-03");
    }

    #[test]
    fn queue_labels_use_slug_and_repo_not_prompt() {
        let item = state::QueueItemRow {
            id: "item-with-sensitive-prompt".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "rotate production credential".to_string(),
            slug: "task-run-01".to_string(),
            repo_root: Some("/workspace/repo".to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };

        assert_eq!(queue_display_label(&item), "repo task-run-01");
        assert_eq!(queue_agent_id(&item), "qa-task-run-01");
        assert!(!queue_display_label(&item).contains("credential"));
    }

    #[test]
    fn queue_slug_deduplicates_with_run_prefix() {
        let mut used = HashSet::new();
        assert_eq!(
            clean_queue_slug("task-run-01", "run", 0, &mut used),
            "task-run-01"
        );
        assert_eq!(
            clean_queue_slug("task-run-01", "run", 1, &mut used),
            "task-run-02"
        );
    }

    #[test]
    fn queue_item_outcome_distinguishes_retryable_launch_failures() {
        match QueueItemOutcome::retryable_failure("terminal setup failed") {
            QueueItemOutcome::Failed { message, retryable } => {
                assert_eq!(message, "terminal setup failed");
                assert!(retryable);
            }
            _ => panic!("expected failed outcome"),
        }

        match QueueItemOutcome::failed("agent exited before task closeout") {
            QueueItemOutcome::Failed { message, retryable } => {
                assert_eq!(message, "agent exited before task closeout");
                assert!(!retryable);
            }
            _ => panic!("expected failed outcome"),
        }
    }

    #[test]
    fn queue_launch_failure_retry_reports_agent_cleanup() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());

        match retry_after_queue_agent_launch_failure(
            "missing-agent",
            "agent terminal did not appear",
        ) {
            QueueItemOutcome::Failed { message, retryable } => {
                assert_eq!(
                    message,
                    "agent terminal did not appear; agent already stopped"
                );
                assert!(retryable);
            }
            _ => panic!("expected failed outcome"),
        }
    }

    #[test]
    fn stopped_queue_item_is_resumable_not_terminal() {
        assert!(!queue_item_terminal("stopped"));
        assert!(!queue_item_terminal("paused"));
        assert!(queue_item_terminal("success"));
        assert!(queue_item_terminal("failed"));
        assert!(queue_item_terminal("blocked"));
    }

    #[test]
    fn stopped_queue_run_can_continue_without_deleting_current_item() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("run-stopped", "stopped", 1);
        run.stop_requested = true;
        let items = vec![
            queue_item_fixture("run-stopped", "first", 0, "success", Some("agent-1")),
            queue_item_fixture("run-stopped", "second", 1, "stopped", Some("agent-2")),
            queue_item_fixture("run-stopped", "third", 2, "pending", None),
        ];

        state::replace_web_queue(&run, &items).unwrap();
        state::continue_web_queue_run("run-stopped").unwrap();
        let (stored_run, stored_items) = state::load_web_queue_run("run-stopped").unwrap();
        let stored_run = stored_run.unwrap();

        assert_eq!(stored_run.status, "running");
        assert!(!stored_run.stop_requested);
        assert_eq!(
            stored_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [
                ("first", "success"),
                ("second", "stopped"),
                ("third", "pending")
            ]
        );
    }

    #[test]
    fn running_queue_removal_contract_covers_active_future_and_terminal_rows() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("run", "running", 3);
        let cases = [
            ("success", 2, None, true),
            ("failed", 2, None, true),
            ("blocked", 2, None, true),
            ("stopped", 2, None, false),
            ("pending", 4, None, true),
            ("waiting", 4, None, true),
            ("pending", 3, None, false),
            ("running", 4, None, false),
            ("waiting", 4, Some("queue-run-1"), false),
        ];

        for (status, position, agent_id, expected) in cases {
            let item = queue_item_fixture("run", "item", position, status, agent_id);
            assert_eq!(
                queue_item_removable_while_running(&run, &item).unwrap(),
                expected,
                "status={status} position={position} agent_id={agent_id:?}"
            );
        }
    }

    #[test]
    fn queue_persistence_contract_preserves_order_and_deletes_empty_run() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("run-contract", "running", 1);
        let mut third = queue_item_fixture("run-contract", "third", 3, "pending", None);
        third.depends_on = vec!["first".to_string()];
        let items = vec![
            third,
            queue_item_fixture("run-contract", "first", 1, "success", Some("agent-1")),
            queue_item_fixture("run-contract", "second", 2, "pending", None),
        ];

        state::replace_web_queue(&run, &items).unwrap();
        let (stored_run, stored_items) = state::load_web_queue_run("run-contract").unwrap();

        assert_eq!(stored_run.unwrap().id, "run-contract");
        assert_eq!(stored_items[2].depends_on, vec!["first".to_string()]);
        assert_eq!(
            stored_items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );

        let deleted = state::delete_web_queue_item("run-contract", "second").unwrap();
        assert_eq!(deleted.position, 2);
        let (_, remaining) = state::load_web_queue_run("run-contract").unwrap();
        assert_eq!(remaining.len(), 2);

        state::delete_web_queue_item("run-contract", "first").unwrap();
        state::delete_web_queue_item("run-contract", "third").unwrap();
        let (empty_run, empty_items) = state::load_web_queue_run("run-contract").unwrap();
        assert!(empty_run.is_none());
        assert!(empty_items.is_empty());
    }

    #[test]
    fn queue_storage_contract_handles_realistic_batch_with_budget() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("run-batch", "running", 20);
        let items = (0..200)
            .map(|index| {
                queue_item_fixture(
                    "run-batch",
                    &format!("item-{index:03}"),
                    index,
                    if index < 20 { "success" } else { "pending" },
                    (index < 20).then_some("agent"),
                )
            })
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();

        state::replace_web_queue(&run, &items).unwrap();
        let (_, loaded) = state::load_web_queue_run("run-batch").unwrap();
        let removable = loaded
            .iter()
            .filter(|item| queue_item_removable_while_running(&run, item).unwrap())
            .count();

        assert_eq!(loaded.len(), 200);
        assert_eq!(removable, 199);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "queue batch contract exceeded budget: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn graph_queue_ready_items_respect_dependencies() {
        let mut run = queue_run_fixture("graph", "running", -1);
        run.execution_mode = "graph".to_string();
        let mut first = queue_item_fixture("graph", "first", 0, "pending", None);
        let second = queue_item_fixture("graph", "second", 1, "pending", None);
        let mut third = queue_item_fixture("graph", "third", 2, "pending", None);
        third.depends_on = vec!["first".to_string(), "second".to_string()];
        let items = vec![first.clone(), second.clone(), third.clone()];

        assert_eq!(
            queue_ready_items(&run, &items)
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );

        first.status = "success".to_string();
        let items = vec![first, second, third];
        assert_eq!(
            queue_ready_items(&run, &items)
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["second"]
        );
    }

    #[test]
    fn graph_queue_ready_items_advance_in_dependency_waves() {
        let mut run = queue_run_fixture("graph-waves", "running", -1);
        run.execution_mode = "graph".to_string();
        let mut plan = vec![
            queue_item_fixture("graph-waves", "bootstrap-a", 0, "pending", None),
            queue_item_fixture("graph-waves", "bootstrap-b", 1, "pending", None),
            queue_item_fixture("graph-waves", "fanout-c", 2, "pending", None),
            queue_item_fixture("graph-waves", "fanout-d", 3, "pending", None),
            queue_item_fixture("graph-waves", "join-e", 4, "pending", None),
            queue_item_fixture("graph-waves", "tail-f", 5, "pending", None),
        ];
        plan[2].depends_on = vec!["bootstrap-a".to_string()];
        plan[3].depends_on = vec!["bootstrap-a".to_string(), "bootstrap-b".to_string()];
        plan[4].depends_on = vec!["fanout-c".to_string(), "fanout-d".to_string()];
        plan[5].depends_on = vec!["join-e".to_string()];

        assert_eq!(
            queue_ready_item_ids(&run, &plan),
            ids(&["bootstrap-a", "bootstrap-b"])
        );

        set_queue_item_status(&mut plan, "bootstrap-a", "success");
        assert_eq!(
            queue_ready_item_ids(&run, &plan),
            ids(&["bootstrap-b", "fanout-c"])
        );

        set_queue_item_status(&mut plan, "bootstrap-b", "success");
        assert_eq!(
            queue_ready_item_ids(&run, &plan),
            ids(&["fanout-c", "fanout-d"])
        );

        set_queue_item_status(&mut plan, "fanout-c", "success");
        assert_eq!(queue_ready_item_ids(&run, &plan), ids(&["fanout-d"]));

        set_queue_item_status(&mut plan, "fanout-d", "success");
        assert_eq!(queue_ready_item_ids(&run, &plan), ids(&["join-e"]));

        set_queue_item_status(&mut plan, "join-e", "success");
        assert_eq!(queue_ready_item_ids(&run, &plan), ids(&["tail-f"]));
    }

    #[test]
    fn graph_queue_reconciles_success_record_and_unblocks_downstream_wave() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-closeout", "running", -1);
        run.execution_mode = "graph".to_string();
        let upstream =
            queue_item_fixture("graph-closeout", "upstream", 0, "running", Some("agent-1"));
        let mut downstream_a =
            queue_item_fixture("graph-closeout", "downstream-a", 1, "pending", None);
        let mut downstream_b =
            queue_item_fixture("graph-closeout", "downstream-b", 2, "pending", None);
        downstream_a.depends_on = vec!["upstream".to_string()];
        downstream_b.depends_on = vec!["upstream".to_string()];
        let items = vec![upstream, downstream_a, downstream_b];
        state::replace_web_queue(&run, &items).unwrap();
        state::upsert_task_record(&state::new_task_record(
            "task/task-upstream".to_string(),
            "task-flow".to_string(),
            "upstream".to_string(),
            "prompt upstream".to_string(),
            "closed:success".to_string(),
            None,
            None,
            Some("agent-1".to_string()),
            None,
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let upstream = stored_items
            .iter()
            .find(|item| item.id == "upstream")
            .unwrap();

        assert_eq!(stored_run.unwrap().status, "running");
        assert_eq!(upstream.status, "success");
        assert!(upstream.message.contains("closed successfully"));
        assert!(upstream.message.contains("agent already stopped"));
        assert_eq!(
            queue_ready_item_ids(&run, &stored_items),
            ids(&["downstream-a", "downstream-b"])
        );
    }

    #[test]
    fn graph_queue_reconciles_closed_success_after_repo_drift() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-drift", "running", -1);
        run.execution_mode = "graph".to_string();
        let mut item = queue_item_fixture("graph-drift", "upstream", 0, "running", Some("agent-1"));
        item.repo_root = Some("/tmp/old-active".to_string());
        let mut dependent = queue_item_fixture("graph-drift", "dependent", 1, "pending", None);
        dependent.depends_on = vec!["upstream".to_string()];
        state::replace_web_queue(&run, &[item, dependent]).unwrap();
        state::upsert_task_record(&state::new_task_record(
            "task/task-upstream".to_string(),
            "task-flow".to_string(),
            "upstream".to_string(),
            "prompt upstream".to_string(),
            "closed:success".to_string(),
            Some("/workspace/repo".to_string()),
            None,
            Some("agent-1".to_string()),
            None,
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();

        assert_eq!(stored_items[0].status, "success");
        assert_eq!(queue_ready_item_ids(&run, &stored_items), ids(&["dependent"]));
    }

    #[test]
    fn failed_graph_queue_reconciles_parallel_started_rows() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("graph-failed-reconcile", "failed", 1);
        run.execution_mode = "graph".to_string();
        let first = queue_item_fixture(&run.id, "first", 0, "success", Some("agent-1"));
        let mut second = queue_item_fixture(&run.id, "second", 1, "failed", Some("agent-2"));
        second.message = "agent reached idle prompt after failed Q-COLD closeout".to_string();
        let third = queue_item_fixture(&run.id, "third", 2, "running", Some("agent-3"));
        state::replace_web_queue(&run, &[first, second, third]).unwrap();
        state::upsert_task_record(&state::new_task_record(
            "task/task-third".to_string(),
            "task-flow".to_string(),
            "third".to_string(),
            "prompt third".to_string(),
            "closed:failed".to_string(),
            None,
            None,
            Some("agent-3".to_string()),
            None,
        ))
        .unwrap();

        reconcile_stale_web_queue_run().unwrap();
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();

        assert_eq!(stored_run.status, "failed");
        assert_eq!(
            stored_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [("first", "success"), ("second", "failed"), ("third", "failed")]
        );
    }

    #[test]
    fn graph_queue_does_not_unblock_dependents_on_failed_or_blocked_prerequisites() {
        for terminal_status in ["failed", "blocked"] {
            let mut run = queue_run_fixture("graph-stop", "running", -1);
            run.execution_mode = "graph".to_string();
            let upstream =
                queue_item_fixture("graph-stop", "upstream", 0, terminal_status, Some("agent-1"));
            let mut dependent = queue_item_fixture("graph-stop", "dependent", 1, "pending", None);
            dependent.depends_on = vec!["upstream".to_string()];
            let independent = queue_item_fixture("graph-stop", "independent", 2, "pending", None);
            let items = vec![upstream, dependent, independent];

            assert_eq!(
                queue_ready_item_ids(&run, &items),
                ids(&["independent"]),
                "terminal prerequisite status {terminal_status} must not satisfy dependencies"
            );
        }
    }

    #[test]
    fn queue_clear_deletes_empty_backend_run() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("empty-run", "stopped", -1);
        state::replace_web_queue(&run, &[]).unwrap();

        let response = handle_queue_clear(
            &HeaderMap::new(),
            &QueueClearRequest {
                run_id: Some(run.id.clone()),
            },
        );

        assert!(response.ok, "{}", response.output);
        assert_eq!(response.output, "cleared 0 queue item(s)");
        assert!(state::load_web_queue_run(&run.id).unwrap().0.is_none());
    }

    #[test]
    fn graph_queue_scheduler_stops_on_non_success_closeout_without_advancing_dependents() {
        for terminal_status in ["failed", "blocked"] {
            let _guard = test_support::env_guard();
            let temp = tempdir().unwrap();
            std::env::set_var("QCOLD_STATE_DIR", temp.path());
            let mut run = queue_run_fixture(&format!("run-{terminal_status}"), "running", -1);
            run.execution_mode = "graph".to_string();
            let mut upstream =
                queue_item_fixture(&run.id, "upstream", 0, terminal_status, Some("agent-1"));
            upstream.message = format!("upstream ended as {terminal_status}");
            let mut dependent = queue_item_fixture(&run.id, "dependent", 1, "pending", None);
            dependent.depends_on = vec!["upstream".to_string()];
            let independent = queue_item_fixture(&run.id, "independent", 2, "pending", None);
            let items = vec![upstream, dependent, independent];

            state::replace_web_queue(&run, &items).unwrap();
            run_web_queue(&run.id).unwrap();
            let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
            let stored_run = stored_run.unwrap();

            assert_eq!(stored_run.status, "failed");
            assert_eq!(stored_run.current_index, 0);
            assert_eq!(stored_run.message, format!("upstream ended as {terminal_status}"));
            assert_eq!(
                stored_items
                    .iter()
                    .map(|item| (item.id.as_str(), item.status.as_str(), item.agent_id.as_deref()))
                    .collect::<Vec<_>>(),
                [
                    ("upstream", terminal_status, Some("agent-1")),
                    ("dependent", "pending", None),
                    ("independent", "pending", None),
                ],
                "scheduler must stop the graph before spawning unrelated or downstream work"
            );
        }
    }

    #[test]
    fn graph_dependency_normalization_keeps_only_valid_unique_prerequisites() {
        let mut items = vec![
            queue_item_fixture("graph", "first", 0, "pending", None),
            queue_item_fixture("graph", "second", 1, "pending", None),
            queue_item_fixture("graph", "third", 2, "pending", None),
        ];
        items[2].depends_on = vec![
            "first".to_string(),
            "missing".to_string(),
            "first".to_string(),
            "third".to_string(),
            "second".to_string(),
        ];

        normalize_queue_dependencies("graph", &mut items).unwrap();

        assert_eq!(
            items[2].depends_on,
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn sequence_dependency_normalization_strips_graph_edges() {
        let mut items = vec![
            queue_item_fixture("sequence", "first", 0, "pending", None),
            queue_item_fixture("sequence", "second", 1, "pending", None),
        ];
        items[1].depends_on = vec!["first".to_string()];

        normalize_queue_dependencies("sequence", &mut items).unwrap();

        assert!(items.iter().all(|item| item.depends_on.is_empty()));
    }

    #[test]
    fn sequence_queue_still_runs_one_ready_item_at_a_time() {
        let run = queue_run_fixture("sequence", "running", -1);
        let items = vec![
            queue_item_fixture("sequence", "first", 0, "pending", None),
            queue_item_fixture("sequence", "second", 1, "pending", None),
        ];

        assert_eq!(
            queue_ready_items(&run, &items)
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["first"]
        );
    }

    #[test]
    fn graph_dependency_normalization_rejects_cycles() {
        let mut items = vec![
            queue_item_fixture("graph", "first", 0, "pending", None),
            queue_item_fixture("graph", "second", 1, "pending", None),
        ];
        items[0].depends_on = vec!["second".to_string()];
        items[1].depends_on = vec!["first".to_string()];

        assert!(normalize_queue_dependencies("graph", &mut items).is_err());
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

    fn set_queue_item_status(items: &mut [state::QueueItemRow], id: &str, status: &str) {
        let item = items
            .iter_mut()
            .find(|item| item.id == id)
            .unwrap_or_else(|| panic!("missing queue item fixture {id}"));
        item.status = status.to_string();
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

    #[test]
    fn terminal_pane_parser_builds_tmux_target() {
        let pane = parse_terminal_pane("main\t0.1\t123\tcodex\t/workspace/repo").unwrap();
        assert_eq!(pane.target, "main:0.1");
        assert_eq!(pane.pid, 123);
        assert_eq!(pane.command, "codex");
        assert_eq!(pane.label, "main - codex");
    }

    #[test]
    fn terminal_capture_uses_deeper_scrollback_window() {
        assert_eq!(terminal_capture_start_arg(), "-2000");
    }

    #[test]
    fn terminal_scrollback_trim_keeps_recent_lines() {
        let output = (0..=TERMINAL_CAPTURE_LINES + 3)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");

        let trimmed = trim_terminal_scrollback(&format!("{output}\n\n"));

        let lines = trimmed.lines().collect::<Vec<_>>();
        let expected_last = format!("line-{}", TERMINAL_CAPTURE_LINES + 3);
        assert_eq!(lines.len(), TERMINAL_CAPTURE_LINES);
        assert_eq!(lines.first(), Some(&"line-4"));
        assert_eq!(lines.last().copied(), Some(expected_last.as_str()));
        assert!(!trimmed.ends_with('\n'));
    }

    #[test]
    fn terminal_command_summary_uses_wrapped_agent_prompt() {
        assert_eq!(
            terminal_command_summary("cc2 \"refactor terminal naming\"").as_deref(),
            Some("refactor terminal naming")
        );
        assert_eq!(
            terminal_command_summary("codex exec \"inspect terminal panes\"").as_deref(),
            Some("inspect terminal panes")
        );
    }

    #[test]
    fn generated_agent_label_does_not_include_prompt_text() {
        let context = agents::TerminalAgentContext {
            id: "queue-run-1234567890".to_string(),
            track: "queue-run".to_string(),
            session: "qcold-queue-run-1234567890".to_string(),
            pane: "0.0".to_string(),
            target: "qcold-queue-run-1234567890:0.0".to_string(),
            started_at: 123,
            command: "codex exec \"rotate production credential\"".to_string(),
        };

        let label = generated_agent_label(&context);

        assert_eq!(label, "queue-run #7890");
        assert!(!label.contains("credential"));
    }

    #[test]
    fn task_chat_resume_packet_reports_existing_state_paths() {
        let temp = tempdir().unwrap();
        let cwd = temp.path().join("task-worktree");
        let session = temp.path().join("session.jsonl");
        fs::create_dir_all(cwd.join(".task/logs")).unwrap();
        fs::write(cwd.join(".task/task.env"), "TASK_ID=task/example\n").unwrap();
        fs::write(&session, "{}\n").unwrap();
        let record = state::TaskRecordRow {
            id: "task/example".to_string(),
            source: "task-flow".to_string(),
            sequence: Some(7),
            title: "example".to_string(),
            description: "operator body".to_string(),
            status: "paused".to_string(),
            created_at: 1,
            updated_at: 2,
            repo_root: Some(temp.path().join("repo").display().to_string()),
            cwd: Some(cwd.display().to_string()),
            agent_id: None,
            metadata_json: Some(
                serde_json::json!({"session_path": session.display().to_string()}).to_string(),
            ),
        };

        let packet = task_chat_resume_packet(&record);

        assert!(packet.contains("Q-COLD_RESUME_PACKET"));
        assert!(packet.contains("task_id: task/example"));
        assert!(packet.contains("task_env: "));
        assert!(packet.contains(".task/task.env"));
        assert!(packet.contains("task_logs: "));
        assert!(packet.contains(".task/logs"));
        assert!(packet.contains(&format!("codex_session_path: {}", session.display())));
        assert!(packet.contains("visible task state only"));
        assert!(!packet.contains("operator body"));
    }

    #[test]
    fn task_chat_resume_packet_omits_stale_state_paths() {
        let record = state::TaskRecordRow {
            id: "task/example".to_string(),
            source: "task-flow".to_string(),
            sequence: Some(7),
            title: "example".to_string(),
            description: "operator body".to_string(),
            status: "paused".to_string(),
            created_at: 1,
            updated_at: 2,
            repo_root: None,
            cwd: Some("/definitely/missing/qcold-task".to_string()),
            agent_id: None,
            metadata_json: Some(
                serde_json::json!({"session_path": "/definitely/missing/session.jsonl"})
                    .to_string(),
            ),
        };

        let packet = task_chat_resume_packet(&record);

        assert!(packet.contains("Q-COLD_RESUME_PACKET"));
        assert!(!packet.contains("cwd: /definitely/missing/qcold-task"));
        assert!(!packet.contains("codex_session_path: /definitely/missing/session.jsonl"));
    }

    #[test]
    fn terminal_metadata_override_becomes_display_label() {
        let mut pane = TerminalPane::new(
            "zellij:qcold-c2-1234:terminal_0".to_string(),
            "qcold-c2-1234".to_string(),
            "terminal_0".to_string(),
            42,
            "c2-1234".to_string(),
            "/repo".to_string(),
        );
        let metadata = state::TerminalMetadataRow {
            target: pane.target.clone(),
            name: Some("client migration".to_string()),
            scope: Some("review".to_string()),
            updated_at: 123,
        };

        apply_terminal_details(&mut pane, None, Some(&metadata));

        assert_eq!(pane.generated_label, "c2-1234 - c2-1234");
        assert_eq!(pane.label, "client migration");
        assert_eq!(pane.name, "client migration");
        assert_eq!(pane.scope, "review");
    }

    #[test]
    fn terminal_metadata_values_are_compacted_and_limited() {
        assert_eq!(
            clean_terminal_metadata_value(Some("  refactoring\n  terminal labels  ")).as_deref(),
            Some("refactoring terminal labels")
        );
        assert_eq!(clean_terminal_metadata_value(Some(" \n\t ")), None);
    }

    #[test]
    fn codex_transcript_messages_include_user_and_agent_text() {
        let temp = tempdir().unwrap();
        let session = temp.path().join("session.jsonl");
        fs::write(
            &session,
            concat!(
                "{\"timestamp\":\"2026-05-10T00:00:00Z\",\"type\":\"event_msg\",",
                "\"payload\":{\"type\":\"user_message\",\"message\":\"fix queue\",\"images\":[]}}\n",
                "{\"timestamp\":\"2026-05-10T00:00:01Z\",\"type\":\"response_item\",",
                "\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[",
                "{\"type\":\"output_text\",\"text\":\"queue fixed\"}]}}\n",
                "{\"timestamp\":\"2026-05-10T00:00:02Z\",\"type\":\"event_msg\",",
                "\"payload\":{\"type\":\"token_count\",\"info\":null}}\n"
            ),
        )
        .unwrap();

        let messages = codex_transcript_messages(&session).unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].text, "fix queue");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].text, "queue fixed");
    }
}
