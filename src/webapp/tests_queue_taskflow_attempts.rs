#[test]
fn closed_failed_queue_task_schedules_one_auto_recovery() {
    let _guard = test_support::env_guard();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let run = queue_run_fixture("auto-recovery", &repo);
    let mut item = queue_taskflow_item("task-auto-recovery", &repo, None);
    item.run_id = run.id.clone();
    item.status = "running".into();
    item.agent_id = Some("agent-failed".to_string());
    state::replace_web_queue(&run, &[item]).unwrap();
    state::upsert_task_record(&task_record_fixture(
        "task-auto-recovery",
        "closed:failed",
        &repo,
    ))
    .unwrap();

    let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    assert!(matches!(
        reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
        QueueReconcile::Changed
    ));
    let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    let stored_run = stored_run.unwrap();
    let recovered = &stored_items[0];

    assert_eq!(stored_run.status, "running");
    assert_eq!(recovered.status, "pending");
    assert_eq!(recovered.recovery_attempts, 1);
    assert!(recovered.agent_id.is_none());
    assert!(recovered.message.contains("auto-recovery scheduled"));
    assert!(recovered.message.contains("closed:failed"));
    assert!(matches!(
        reconcile_queue_task_statuses(&stored_run, &stored_items).unwrap(),
        QueueReconcile::Unchanged
    ));
    let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    assert_eq!(stored_items[0].status, "pending");
    assert_eq!(stored_items[0].recovery_attempts, 1);
    let attempts = state::load_web_queue_item_attempts(&run.id, &recovered.id).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].semantic_iteration, 1);
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(
        attempts[0].failure_message.as_deref(),
        Some("closed:failed")
    );
    assert_eq!(attempts[1].semantic_iteration, 2);
    assert_eq!(attempts[1].status, "pending");
}

#[test]
fn closed_failed_queue_task_after_first_auto_recovery_schedules_second_recovery() {
    let _guard = test_support::env_guard();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let run = queue_run_fixture("auto-recovery-exhausted", &repo);
    let mut item = queue_taskflow_item("task-auto-recovery-exhausted", &repo, None);
    item.run_id = run.id.clone();
    item.status = "running".into();
    item.agent_id = Some("agent-recovery".to_string());
    item.recovery_attempts = 1;
    state::replace_web_queue(&run, &[item]).unwrap();
    state::upsert_task_record(&task_record_fixture(
        "task-auto-recovery-exhausted",
        "closed:failed",
        &repo,
    ))
    .unwrap();

    let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    assert!(matches!(
        reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
        QueueReconcile::Changed
    ));
    let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    let recovered = &stored_items[0];

    assert_eq!(stored_run.unwrap().status, "running");
    assert_eq!(recovered.status, "pending");
    assert_eq!(recovered.recovery_attempts, 2);
    assert!(recovered.message.contains("auto-recovery scheduled"));
    let attempts = state::load_web_queue_item_attempts(&run.id, &recovered.id).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].semantic_iteration, 2);
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(attempts[1].semantic_iteration, 3);
    assert_eq!(attempts[1].status, "pending");
}

#[test]
fn closed_failed_queue_task_after_second_auto_recovery_remains_failed() {
    let _guard = test_support::env_guard();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let run = queue_run_fixture("auto-recovery-exhausted", &repo);
    let mut item = queue_taskflow_item("task-auto-recovery-exhausted", &repo, None);
    item.run_id = run.id.clone();
    item.status = "running".into();
    item.agent_id = Some("agent-recovery".to_string());
    item.recovery_attempts = 2;
    state::replace_web_queue(&run, &[item]).unwrap();
    state::upsert_task_record(&task_record_fixture(
        "task-auto-recovery-exhausted",
        "closed:failed",
        &repo,
    ))
    .unwrap();

    let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    assert!(matches!(
        reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
        QueueReconcile::Terminal
    ));
    let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    let failed = &stored_items[0];

    assert_eq!(stored_run.unwrap().status, "failed");
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.recovery_attempts, 2);
    assert_eq!(failed.message, "closed:failed");
    let attempts = state::load_web_queue_item_attempts(&run.id, &failed.id).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].semantic_iteration, 3);
    assert_eq!(attempts[0].status, "failed");
}

#[test]
fn failed_closeout_queue_task_schedules_auto_recovery() {
    let _guard = test_support::env_guard();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let run = queue_run_fixture("failed-closeout", &repo);
    let mut item = queue_taskflow_item("task-failed-closeout", &repo, None);
    item.run_id = run.id.clone();
    item.status = "running".into();
    item.execution_host = "remote-native".into();
    item.agent_id = Some("qa-task-failed-closeout".to_string());
    state::replace_web_queue(&run, &[item]).unwrap();
    state::upsert_task_record(&task_record_fixture(
        "task-failed-closeout",
        "failed-closeout",
        &repo,
    ))
    .unwrap();

    let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    assert!(matches!(
        reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
        QueueReconcile::Changed
    ));
    let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    let stored_run = stored_run.unwrap();
    let recovered = &stored_items[0];

    assert_eq!(stored_run.status, "running");
    assert_eq!(recovered.status, "pending");
    assert_eq!(recovered.recovery_attempts, 1);
    assert_eq!(recovered.agent_id.as_deref(), None);
    assert!(recovered.message.contains("auto-recovery scheduled"));
    assert!(recovered.message.contains("failed-closeout"));
    let attempts = state::load_web_queue_item_attempts(&run.id, &recovered.id).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].semantic_iteration, 1);
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(
        attempts[0].failure_message.as_deref(),
        Some("failed-closeout")
    );
    assert_eq!(attempts[1].semantic_iteration, 2);
    assert_eq!(attempts[1].status, "pending");
}

#[test]
fn failed_closeout_after_second_auto_recovery_remains_failed() {
    let _guard = test_support::env_guard();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let run = queue_run_fixture("auto-recovery-failed-closeout-exhausted", &repo);
    let mut item = queue_taskflow_item(
        "task-auto-recovery-failed-closeout-exhausted",
        &repo,
        None,
    );
    item.run_id = run.id.clone();
    item.status = "running".into();
    item.agent_id = Some("agent-recovery".to_string());
    item.recovery_attempts = 2;
    state::replace_web_queue(&run, &[item]).unwrap();
    state::upsert_task_record(&task_record_fixture(
        "task-auto-recovery-failed-closeout-exhausted",
        "failed-closeout",
        &repo,
    ))
    .unwrap();

    let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    assert!(matches!(
        reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
        QueueReconcile::Terminal
    ));
    let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    let failed = &stored_items[0];

    assert_eq!(stored_run.unwrap().status, "failed");
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.recovery_attempts, 2);
    assert_eq!(failed.message, "failed-closeout");
    let attempts = state::load_web_queue_item_attempts(&run.id, &failed.id).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].semantic_iteration, 3);
    assert_eq!(attempts[0].status, "failed");
}

#[test]
fn stale_failed_agent_exit_row_schedules_auto_recovery() {
    let _guard = test_support::env_guard();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let mut run = queue_run_fixture("stale-agent-exit-recovery", &repo);
    run.status = "failed".into();
    run.message = QUEUE_AGENT_EXITED_BEFORE_CLOSEOUT.to_string();
    let mut item = queue_taskflow_item("task-stale-agent-exit-recovery", &repo, None);
    item.run_id = run.id.clone();
    item.status = "failed".into();
    item.message = QUEUE_AGENT_EXITED_BEFORE_CLOSEOUT.to_string();
    item.agent_id = Some("agent-exited".to_string());
    state::replace_web_queue(&run, &[item]).unwrap();

    let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    assert!(failed_queue_run_may_be_resolved(&run, &stored_items).unwrap());
    assert!(matches!(
        reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
        QueueReconcile::Changed
    ));
    let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    let recovered = &stored_items[0];

    assert_eq!(stored_run.unwrap().status, "running");
    assert_eq!(recovered.status, "pending");
    assert_eq!(recovered.agent_id.as_deref(), None);
    assert_eq!(recovered.recovery_attempts, 1);
    assert!(recovered.message.contains("auto-recovery scheduled"));
    assert!(recovered.message.contains(QUEUE_AGENT_EXITED_BEFORE_CLOSEOUT));
}
