#[cfg(test)]
mod queue_state_model_tests {
    #![allow(clippy::unwrap_used)]

    use crate::{state, test_support};

    use super::*;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    const TARGET_SEMANTIC_ITERATIONS_PER_ITEM: i64 = 3;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TargetItemStatus {
        Ready,
        WaitingForLaunchRetry,
        Running,
        Success,
        Failed,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct TargetItemState {
        status: TargetItemStatus,
        semantic_iterations_started: i64,
        launch_retries: i64,
    }

    impl TargetItemState {
        fn initial_ready() -> Self {
            Self {
                status: TargetItemStatus::Ready,
                semantic_iterations_started: 1,
                launch_retries: 0,
            }
        }

        fn launch_failed(mut self) -> Self {
            self.status = TargetItemStatus::WaitingForLaunchRetry;
            self.launch_retries += 1;
            self
        }

        fn semantic_failed(mut self) -> Self {
            if self.semantic_iterations_started < TARGET_SEMANTIC_ITERATIONS_PER_ITEM {
                self.semantic_iterations_started += 1;
                self.status = TargetItemStatus::Ready;
            } else {
                self.status = TargetItemStatus::Failed;
            }
            self
        }

        fn reconcile_live_agent(mut self) -> Self {
            self.status = TargetItemStatus::Running;
            self
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TargetGraphItem {
        id: &'static str,
        status: TargetItemStatus,
        depends_on: Vec<&'static str>,
    }

    fn target_ready_graph_items(items: &[TargetGraphItem]) -> Vec<&'static str> {
        items
            .iter()
            .filter(|item| item.status == TargetItemStatus::Ready)
            .filter(|item| {
                item.depends_on.iter().all(|dependency| {
                    items.iter().any(|candidate| {
                        candidate.id == *dependency
                            && candidate.status == TargetItemStatus::Success
                    })
                })
            })
            .map(|item| item.id)
            .collect()
    }

    fn target_admitted_ready_items(
        ready: &[&'static str],
        active_count: usize,
        max_active: usize,
    ) -> Vec<&'static str> {
        let capacity = max_active.saturating_sub(active_count);
        ready.iter().take(capacity).copied().collect()
    }

    #[test]
    fn target_model_caps_failed_item_at_three_semantic_iterations() {
        let after_first_failure = TargetItemState::initial_ready().semantic_failed();
        let after_second_failure = after_first_failure.semantic_failed();
        let after_third_failure = after_second_failure.semantic_failed();

        assert_eq!(after_first_failure.status, TargetItemStatus::Ready);
        assert_eq!(after_first_failure.semantic_iterations_started, 2);
        assert_eq!(after_second_failure.status, TargetItemStatus::Ready);
        assert_eq!(
            after_second_failure.semantic_iterations_started,
            TARGET_SEMANTIC_ITERATIONS_PER_ITEM
        );
        assert_eq!(after_third_failure.status, TargetItemStatus::Failed);
        assert_eq!(
            after_third_failure.semantic_iterations_started,
            TARGET_SEMANTIC_ITERATIONS_PER_ITEM
        );
    }

    #[test]
    fn target_model_launch_retry_does_not_consume_semantic_iteration() {
        let state = TargetItemState::initial_ready().launch_failed();

        assert_eq!(state.status, TargetItemStatus::WaitingForLaunchRetry);
        assert_eq!(state.launch_retries, 1);
        assert_eq!(state.semantic_iterations_started, 1);
    }

    #[test]
    fn target_model_daemon_restart_observes_live_agent_without_new_attempt() {
        let before_restart = TargetItemState {
            status: TargetItemStatus::Running,
            semantic_iterations_started: 2,
            launch_retries: 1,
        };
        let after_restart = before_restart.reconcile_live_agent();

        assert_eq!(after_restart, before_restart);
    }

    #[test]
    fn target_model_reconcile_is_idempotent_for_stable_live_agent() {
        let after_one_reconcile = TargetItemState::initial_ready().reconcile_live_agent();
        let after_two_reconciles = after_one_reconcile.reconcile_live_agent();

        assert_eq!(after_two_reconciles, after_one_reconcile);
    }

    #[test]
    fn target_model_graph_admission_limits_ready_fanout() {
        let items = vec![
            TargetGraphItem {
                id: "bootstrap",
                status: TargetItemStatus::Success,
                depends_on: Vec::new(),
            },
            TargetGraphItem {
                id: "fanout-a",
                status: TargetItemStatus::Ready,
                depends_on: vec!["bootstrap"],
            },
            TargetGraphItem {
                id: "fanout-b",
                status: TargetItemStatus::Ready,
                depends_on: vec!["bootstrap"],
            },
        ];

        let ready = target_ready_graph_items(&items);

        assert_eq!(ready, vec!["fanout-a", "fanout-b"]);
        assert_eq!(target_admitted_ready_items(&ready, 1, 2), vec!["fanout-a"]);
    }

    #[test]
    fn runtime_launch_retry_counter_is_separate_from_semantic_recovery_counter() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("launch-retry", "running", 0);
        let mut item = queue_item_fixture(&run.id, "first", 0, "starting", None);
        item.recovery_attempts = 1;
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let mut retries = item.attempts;
        let outcome = handle_queue_launch_outcome(
            &run.id,
            &mut item,
            &mut retries,
            QueueItemOutcome::retryable_failure(CODEX_UPDATE_RESTART_RETRY),
        )
        .unwrap();

        assert!(outcome.is_none());
        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored = &stored_items[0];
        assert_eq!(stored.status, "waiting");
        assert_eq!(stored.attempts, 1);
        assert_eq!(stored.recovery_attempts, 1);
        assert!(stored.next_attempt_at.is_none());
    }

    #[test]
    fn runtime_continue_wakes_delayed_retry_item() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("wake-delayed-retry", "running", 0);
        let mut item = queue_item_fixture(&run.id, "first", 0, "waiting", Some("qa-first"));
        item.message = "retry 3/3 in 600s".to_string();
        item.next_attempt_at = Some(unix_now().saturating_add(600));
        state::replace_web_queue(&run, &[item]).unwrap();

        let woke = state::wake_web_queue_retry_items(&run.id).unwrap();
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let ready = queue_ready_items(&stored_run, &stored_items)
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();

        assert_eq!(woke, 1);
        assert_eq!(stored_run.status, "running");
        assert_eq!(stored_run.message, "continued");
        assert_eq!(stored_items[0].status, "waiting");
        assert_eq!(stored_items[0].agent_id.as_deref(), Some("qa-first"));
        assert!(stored_items[0].next_attempt_at.is_none());
        assert_eq!(ready, vec!["first"]);
    }

    #[test]
    fn runtime_retry_sleep_exits_when_item_retry_was_awakened() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("sleep-wake", "running", 0);
        let item = queue_item_fixture(&run.id, "first", 0, "waiting", Some("qa-first"));
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let started = Instant::now();
        assert!(sleep_queue_retry(&run.id, &item.id, 60).unwrap());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn runtime_worker_lease_acquire_blocks_conflicting_owner() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("lease-conflict", "running", 0);
        let item = queue_item_fixture(&run.id, "first", 0, "pending", None);
        state::replace_web_queue(&run, &[item]).unwrap();

        let lease = match state::acquire_web_queue_item_worker_lease_at(
            &run.id,
            "first",
            "worker-a",
            100,
            30,
        )
        .unwrap()
        {
            state::QueueWorkerLeaseAcquire::Acquired(lease) => lease,
            other => panic!("expected acquired lease, got {other:?}"),
        };
        let blocked = state::acquire_web_queue_item_worker_lease_at(
            &run.id,
            "first",
            "worker-b",
            101,
            30,
        )
        .unwrap();

        assert_eq!(lease.owner_id, "worker-a");
        assert_eq!(lease.lease_epoch, 1);
        assert_eq!(
            state::inspect_web_queue_item_worker_lease_at(&run.id, "first", 101).unwrap(),
            state::QueueWorkerLeaseState::Active {
                owner_id: "worker-a".to_string(),
                lease_epoch: 1,
                expires_at: 130,
            }
        );
        assert_eq!(
            blocked,
            state::QueueWorkerLeaseAcquire::Busy {
                owner_id: "worker-a".to_string(),
                lease_epoch: 1,
                expires_at: 130,
            }
        );
    }

    #[test]
    fn runtime_worker_lease_recovers_stale_owner_deterministically() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("lease-stale", "running", 0);
        let item = queue_item_fixture(&run.id, "first", 0, "pending", None);
        state::replace_web_queue(&run, &[item]).unwrap();

        let first = match state::acquire_web_queue_item_worker_lease_at(
            &run.id,
            "first",
            "worker-a",
            100,
            10,
        )
        .unwrap()
        {
            state::QueueWorkerLeaseAcquire::Acquired(lease) => lease,
            other => panic!("expected acquired lease, got {other:?}"),
        };
        assert!(state::heartbeat_web_queue_item_worker_lease_at(&first, 105, 10).unwrap());
        assert_eq!(
            state::inspect_web_queue_item_worker_lease_at(&run.id, "first", 116).unwrap(),
            state::QueueWorkerLeaseState::Stale {
                owner_id: "worker-a".to_string(),
                lease_epoch: 1,
                expires_at: 115,
            }
        );

        let second = match state::acquire_web_queue_item_worker_lease_at(
            &run.id,
            "first",
            "worker-b",
            116,
            20,
        )
        .unwrap()
        {
            state::QueueWorkerLeaseAcquire::Acquired(lease) => lease,
            other => panic!("expected stale takeover, got {other:?}"),
        };

        assert_eq!(second.owner_id, "worker-b");
        assert_eq!(second.lease_epoch, 2);
        assert!(second.recovered_stale);
        assert_eq!(
            state::inspect_web_queue_item_worker_lease_at(&run.id, "first", 117).unwrap(),
            state::QueueWorkerLeaseState::Active {
                owner_id: "worker-b".to_string(),
                lease_epoch: 2,
                expires_at: 136,
            }
        );
    }

    #[test]
    fn runtime_stale_recovery_preserves_future_retry_lease() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("lease-retry", "running", 0);
        let item = queue_item_fixture(&run.id, "first", 0, "pending", None);
        state::replace_web_queue(&run, &[item]).unwrap();

        let lease = match state::acquire_web_queue_item_worker_lease_at(
            &run.id,
            "first",
            "worker-a",
            100,
            10,
        )
        .unwrap()
        {
            state::QueueWorkerLeaseAcquire::Acquired(lease) => lease,
            other => panic!("expected acquired lease, got {other:?}"),
        };
        state::schedule_web_queue_item_relaunch(&run.id, "first", "retry later", 1, 200)
            .unwrap();

        assert_eq!(
            state::recover_stale_web_queue_item_worker_leases_at(&run.id, 150).unwrap(),
            0
        );
        assert_eq!(
            state::inspect_web_queue_item_worker_lease_at(&run.id, "first", 150).unwrap(),
            state::QueueWorkerLeaseState::Retryable {
                next_attempt_at: 200
            }
        );
        assert_eq!(
            state::recover_stale_web_queue_item_worker_leases_at(&run.id, 201).unwrap(),
            1
        );
        assert!(state::heartbeat_web_queue_item_worker_lease_at(&lease, 202, 10).unwrap());
        assert_eq!(
            state::inspect_web_queue_item_worker_lease_at(&run.id, "first", 203).unwrap(),
            state::QueueWorkerLeaseState::Active {
                owner_id: "worker-a".to_string(),
                lease_epoch: 1,
                expires_at: 212,
            }
        );
    }

    #[test]
    fn runtime_worker_lease_inspects_retryable_and_terminal_items() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("lease-terminal", "running", 0);
        let mut retryable = queue_item_fixture(&run.id, "retryable", 0, "waiting", None);
        retryable.next_attempt_at = Some(200);
        let terminal = queue_item_fixture(&run.id, "terminal", 1, "success", Some("agent-done"));
        state::replace_web_queue(&run, &[retryable, terminal]).unwrap();

        assert_eq!(
            state::inspect_web_queue_item_worker_lease_at(&run.id, "retryable", 100).unwrap(),
            state::QueueWorkerLeaseState::Retryable {
                next_attempt_at: 200
            }
        );
        assert_eq!(
            state::acquire_web_queue_item_worker_lease_at(
                &run.id,
                "retryable",
                "worker-a",
                100,
                30,
            )
            .unwrap(),
            state::QueueWorkerLeaseAcquire::Retryable {
                next_attempt_at: 200
            }
        );
        assert_eq!(
            state::acquire_web_queue_item_worker_lease_at(
                &run.id,
                "terminal",
                "worker-a",
                100,
                30,
            )
            .unwrap(),
            state::QueueWorkerLeaseAcquire::Terminal {
                status: "success".into()
            }
        );
        assert_eq!(
            state::recover_stale_web_queue_item_worker_leases_at(&run.id, 1_000).unwrap(),
            0
        );
    }

    #[test]
    fn runtime_reconcile_restart_preserves_live_agent_and_attempts() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("restart-live-agent", "running", 0);
        let mut item = queue_item_fixture(&run.id, "first", 0, "running", Some("qa-live"));
        item.attempts = 2;
        item.recovery_attempts = 1;
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::insert_agent(&agent_fixture("qa-live", &run.id)).unwrap();

        reconcile_one_stale_web_queue_run(run, vec![item]).unwrap();

        let (_, stored_items) = state::load_web_queue_run("restart-live-agent").unwrap();
        let agents = state::load_agents(&temp.path().join("legacy-agents.tsv")).unwrap();
        let stored = &stored_items[0];
        assert_eq!(stored.status, "running");
        assert_eq!(stored.agent_id.as_deref(), Some("qa-live"));
        assert_eq!(stored.attempts, 2);
        assert_eq!(stored.recovery_attempts, 1);
        assert_eq!(agents.iter().filter(|agent| agent.id == "qa-live").count(), 1);
    }

    #[test]
    fn runtime_reconcile_is_idempotent_for_live_agent_row() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("reconcile-idempotent", "running", 0);
        let item = queue_item_fixture(&run.id, "first", 0, "running", Some("qa-live"));
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::insert_agent(&agent_fixture("qa-live", &run.id)).unwrap();

        reconcile_one_stale_web_queue_run(run.clone(), vec![item.clone()]).unwrap();
        let first_projection = queue_projection(&run.id);
        let (run_after_first, items_after_first) = state::load_web_queue_run(&run.id).unwrap();
        reconcile_one_stale_web_queue_run(run_after_first.unwrap(), items_after_first).unwrap();

        assert_eq!(queue_projection(&run.id), first_projection);
    }

    #[test]
    fn runtime_graph_ready_items_characterize_current_unadmitted_fanout() {
        let mut run = queue_run_fixture("graph-unadmitted", "running", -1);
        run.execution_mode = "graph".into();
        let mut bootstrap = queue_item_fixture(&run.id, "bootstrap", 0, "success", None);
        bootstrap.agent_id = Some("qa-bootstrap".to_string());
        let mut fanout_a = queue_item_fixture(&run.id, "fanout-a", 1, "pending", None);
        fanout_a.depends_on = vec!["bootstrap".to_string()];
        let mut fanout_b = queue_item_fixture(&run.id, "fanout-b", 2, "pending", None);
        fanout_b.depends_on = vec!["bootstrap".to_string()];

        let ready = queue_ready_items(&run, &[bootstrap, fanout_a, fanout_b])
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();

        assert_eq!(ready, vec!["fanout-a", "fanout-b"]);
    }

    #[derive(Debug, PartialEq, Eq)]
    struct QueueProjection {
        run_status: String,
        run_current_index: i64,
        run_message: String,
        item_status: String,
        item_agent_id: Option<String>,
        item_attempts: i64,
        item_recovery_attempts: i64,
    }

    fn queue_projection(run_id: &str) -> QueueProjection {
        let (run, items) = state::load_web_queue_run(run_id).unwrap();
        let run = run.unwrap();
        let item = &items[0];
        QueueProjection {
            run_status: run.status.to_string(),
            run_current_index: run.current_index,
            run_message: run.message,
            item_status: item.status.to_string(),
            item_agent_id: item.agent_id.clone(),
            item_attempts: item.attempts,
            item_recovery_attempts: item.recovery_attempts,
        }
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
