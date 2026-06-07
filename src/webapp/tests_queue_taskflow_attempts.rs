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
    assert!(failed.message.contains("auto-recovery exhausted"));
    assert!(failed.message.contains("3 semantic iterations"));
    assert!(failed.message.contains("closed:failed"));
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
    assert!(failed.message.contains("auto-recovery exhausted"));
    assert!(failed.message.contains("3 semantic iterations"));
    assert!(failed.message.contains("failed-closeout"));
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

#[test]
fn recovery_task_packet_is_one_shot_and_uses_separate_agent_id() {
    let _guard = test_support::env_guard();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let run = queue_run_fixture("auto-recovery-packet", &repo);
    let mut item = queue_taskflow_item("task-auto-recovery-packet", &repo, None);
    item.run_id.clone_from(&run.id);
    item.status = "running".into();
    let first_agent = queue_agent_id(&item);
    item.agent_id = Some(first_agent.clone());
    state::insert_agent(&state::AgentRow {
        id: first_agent.clone(),
        track: queue_track(&run.id),
        pid: 42,
        started_at: 10,
        command: vec!["c1".to_string()],
        cwd: Some(repo.clone()),
        stdout_log_path: Some(repo.join("logs/original.out")),
        stderr_log_path: Some(repo.join("logs/original.err")),
    })
    .unwrap();
    let mut record = task_record_fixture("task-auto-recovery-packet", "closed:failed", &repo);
    record.agent_id = Some(first_agent.clone());
    record.metadata_json =
        Some(r#"{"task_terminal_bundle":"/bundles/task-auto-recovery-packet-failed.zip"}"#.to_string());
    state::upsert_task_record(&record).unwrap();
    state::replace_web_queue(&run, &[item.clone()]).unwrap();
    state::set_web_queue_item_attempt_terminal(&run.id, &item.id, 1, "tmux:first").unwrap();
    state::schedule_web_queue_item_recovery(
        &run.id,
        &item.id,
        "auto-recovery scheduled after failed task: closed:failed",
        "closed:failed",
        1,
    )
    .unwrap();
    let (_, items) = state::load_web_queue_run(&run.id).unwrap();
    let item = &items[0];

    let packet = queue_task_instruction(item);

    assert_ne!(queue_agent_id(item), first_agent);
    assert!(packet.contains("auto_recovery:"));
    assert!(packet.contains("attempt: 1/2"));
    assert!(packet.contains("make one repair attempt"));
    assert!(packet.contains("previous_failure:"));
    assert!(packet.contains("closed:failed"));
    assert!(packet.contains("persisted_attempts:"));
    assert!(packet.contains("iteration: 1"));
    assert!(packet.contains("status: failed"));
    assert!(packet.contains("selected_command: c1"));
    assert!(packet.contains("task_record_id: task/task-auto-recovery-packet"));
    assert!(packet.contains("agent_id: qa-task-auto-recovery-packet"));
    assert!(packet.contains("terminal: tmux:first"));
    assert!(packet.contains("stdout_log: "));
    assert!(packet.contains("logs/original.out"));
    assert!(packet.contains("stderr_log: "));
    assert!(packet.contains("logs/original.err"));
    assert!(packet.contains("bundle: /bundles/task-auto-recovery-packet-failed.zip"));
    assert!(packet.contains("failure_message: |"));
    assert!(packet.contains("iteration: 2"));
    assert!(packet.contains("status: pending"));
}
