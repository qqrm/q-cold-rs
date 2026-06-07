#[test]
fn queue_continue_resumes_failed_local_row_with_open_task_record() {
    let _guard = test_support::env_guard();
    let temp = tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let mut run = queue_run_fixture("graph-continue-open-local-task", "failed", 0);
    run.execution_mode = "graph".into();
    run.message = "agent exited before task closeout".to_string();
    let mut first = queue_item_fixture(&run.id, "first", 0, "failed", Some("qa-task-first"));
    first.message = "agent exited before task closeout".to_string();
    let mut second = queue_item_fixture(&run.id, "second", 1, "pending", None);
    second.depends_on = vec!["first".to_string()];
    state::replace_web_queue(&run, &[first, second]).unwrap();
    state::upsert_task_record(&state::new_task_record(
        "task/task-first".to_string(),
        "task-flow".to_string(),
        "first".to_string(),
        "prompt first".to_string(),
        "open".to_string(),
        None,
        None,
        Some("qa-task-first".to_string()),
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
    assert_eq!(stored_items[0].id, "first");
    assert_eq!(stored_items[0].status, "pending");
    assert_eq!(stored_items[0].agent_id.as_deref(), None);
    assert_eq!(stored_items[0].recovery_attempts, 1);
    assert!(stored_items[0].message.contains("auto-recovery scheduled"));
    assert!(stored_items[0]
        .message
        .contains(LOCAL_OPEN_RECORD_RECOVERY_MESSAGE));
    assert_eq!(stored_items[1].id, "second");
    assert_eq!(stored_items[1].status, "pending");
    assert!(stored_items[1].message.is_empty());
    assert_eq!(stored_items[1].agent_id.as_deref(), None);
    assert!(test_web_queue_worker_spawned(&run.id));
}

#[test]
fn queue_continue_resets_stopped_local_row_with_stale_agent_id() {
    let _guard = test_support::env_guard();
    let temp = tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let mut run = queue_run_fixture("stopped-local-stale-agent", "stopped", 0);
    run.execution_mode = "graph".into();
    run.message = LOCAL_OPEN_RECORD_STOPPED_MESSAGE.to_string();
    let first = queue_item_fixture(&run.id, "first", 0, "stopped", Some("qa-task-first"));
    state::replace_web_queue(&run, std::slice::from_ref(&first)).unwrap();
    state::upsert_task_record(&state::new_task_record(
        "task/task-first".to_string(),
        "task-flow".to_string(),
        "first".to_string(),
        "prompt first".to_string(),
        "open".to_string(),
        None,
        None,
        Some("qa-task-first".to_string()),
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
            .map(|item| {
                (
                    item.id.as_str(),
                    item.status.as_str(),
                    item.message.as_str(),
                    item.agent_id.as_deref(),
                )
            })
            .collect::<Vec<_>>(),
        [("first", "pending", "pending after queue continue", None)]
    );
    assert!(test_web_queue_worker_spawned(&run.id));
}

#[test]
fn queue_continue_resets_stopped_local_row_without_agent_id() {
    let _guard = test_support::env_guard();
    let temp = tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let mut run = queue_run_fixture("stopped-local-no-agent", "stopped", 0);
    run.execution_mode = "graph".into();
    run.message = LOCAL_OPEN_RECORD_STOPPED_MESSAGE.to_string();
    let first = queue_item_fixture(&run.id, "first", 0, "stopped", None);
    state::replace_web_queue(&run, std::slice::from_ref(&first)).unwrap();
    state::upsert_task_record(&state::new_task_record(
        "task/task-first".to_string(),
        "task-flow".to_string(),
        "first".to_string(),
        "prompt first".to_string(),
        "open".to_string(),
        None,
        None,
        Some("qa-task-first".to_string()),
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
            .map(|item| {
                (
                    item.id.as_str(),
                    item.status.as_str(),
                    item.message.as_str(),
                    item.agent_id.as_deref(),
                )
            })
            .collect::<Vec<_>>(),
        [("first", "pending", "pending after queue continue", None)]
    );
    assert!(matches!(
        reconcile_queue_task_statuses(&stored_run, &stored_items).unwrap(),
        QueueReconcile::Unchanged
    ));
    assert!(test_web_queue_worker_spawned(&run.id));
}

#[test]
fn local_open_task_record_without_live_agent_schedules_recovery() {
    let _guard = test_support::env_guard();
    let temp = tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let run = queue_run_fixture("local-open-task-record", "running", 0);
    let item = queue_item_fixture(&run.id, "first", 0, "running", Some("qa-task-first"));
    state::replace_web_queue(&run, std::slice::from_ref(&item)).unwrap();

    let outcome = queue_item_status_closeout_outcome(
        &run.id,
        &item,
        "qa-task-first",
        item.attempts,
        "open".to_string(),
    )
    .unwrap()
    .unwrap();
    let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    let stored_run = stored_run.unwrap();
    let recovered = &stored_items[0];

    assert!(matches!(outcome, QueueItemOutcome::RecoveryScheduled));
    assert_eq!(stored_run.status, "running");
    assert_eq!(recovered.status, "pending");
    assert_eq!(recovered.agent_id.as_deref(), None);
    assert_eq!(recovered.recovery_attempts, 1);
    assert!(recovered.message.contains("auto-recovery scheduled"));
    assert!(recovered.message.contains(LOCAL_OPEN_RECORD_RECOVERY_MESSAGE));
}

#[test]
fn stopped_local_open_task_record_schedules_recovery() {
    let _guard = test_support::env_guard();
    let temp = tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let run = queue_run_fixture("stopped-local-open-task-record", "running", 0);
    let mut item = queue_item_fixture(&run.id, "first", 0, "stopped", Some("qa-task-first"));
    item.message = LOCAL_OPEN_RECORD_STOPPED_MESSAGE.to_string();
    state::replace_web_queue(&run, std::slice::from_ref(&item)).unwrap();
    state::upsert_task_record(&state::new_task_record(
        "task/task-first".to_string(),
        "task-flow".to_string(),
        "first".to_string(),
        "prompt first".to_string(),
        "open".to_string(),
        None,
        None,
        Some("qa-task-first".to_string()),
        None,
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
    assert_eq!(recovered.agent_id.as_deref(), None);
    assert_eq!(recovered.recovery_attempts, 1);
    assert!(recovered.message.contains("auto-recovery scheduled"));
    assert!(recovered.message.contains(LOCAL_OPEN_RECORD_RECOVERY_MESSAGE));
    assert!(matches!(
        reconcile_queue_task_statuses(&stored_run, &stored_items).unwrap(),
        QueueReconcile::Unchanged
    ));
    assert_eq!(queue_ready_item_ids(&stored_run, &stored_items), ids(&["first"]));
}

#[test]
fn stopped_run_with_local_open_task_record_schedules_recovery() {
    let _guard = test_support::env_guard();
    let temp = tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let mut run = queue_run_fixture("stopped-local-open-task-record-run", "stopped", 0);
    run.message = LOCAL_OPEN_RECORD_STOPPED_MESSAGE.to_string();
    let mut item = queue_item_fixture(&run.id, "first", 0, "stopped", Some("qa-task-first"));
    item.message = LOCAL_OPEN_RECORD_STOPPED_MESSAGE.to_string();
    state::replace_web_queue(&run, std::slice::from_ref(&item)).unwrap();
    state::upsert_task_record(&state::new_task_record(
        "task/task-first".to_string(),
        "task-flow".to_string(),
        "first".to_string(),
        "prompt first".to_string(),
        "open".to_string(),
        None,
        None,
        Some("qa-task-first".to_string()),
        None,
    ))
    .unwrap();

    let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    assert!(queue_run_needs_stale_reconcile(&run, &stored_items).unwrap());
    reconcile_one_stale_web_queue_run(run, stored_items).unwrap();
    let (stored_run, stored_items) = state::load_web_queue_run("stopped-local-open-task-record-run")
        .unwrap();
    let stored_run = stored_run.unwrap();
    let recovered = &stored_items[0];

    assert_eq!(stored_run.status, "running");
    assert_eq!(recovered.status, "pending");
    assert_eq!(recovered.recovery_attempts, 1);
    assert!(recovered.message.contains(LOCAL_OPEN_RECORD_RECOVERY_MESSAGE));
    assert!(test_web_queue_worker_spawned(&stored_run.id));
}

#[test]
fn local_open_task_record_with_active_worker_lease_is_launch_in_progress() {
    let _guard = test_support::env_guard();
    let temp = tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let run = queue_run_fixture("local-open-task-record-worker-active", "running", 0);
    let mut item = queue_item_fixture(&run.id, "first", 0, "starting", None);
    item.message = "starting clean agent context".to_string();
    state::replace_web_queue(&run, std::slice::from_ref(&item)).unwrap();
    state::upsert_task_record(&state::new_task_record(
        "task/task-first".to_string(),
        "task-flow".to_string(),
        "first".to_string(),
        "prompt first".to_string(),
        "open".to_string(),
        None,
        None,
        None,
        None,
    ))
    .unwrap();
    let lease = match state::acquire_web_queue_item_worker_lease(&run.id, &item.id, "worker", 120)
        .unwrap()
    {
        state::QueueWorkerLeaseAcquire::Acquired(lease) => lease,
        other => panic!("expected acquired lease, got {other:?}"),
    };

    let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    assert!(matches!(
        reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
        QueueReconcile::Unchanged
    ));
    let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();

    assert_eq!(stored_items[0].status, "starting");
    assert_eq!(stored_items[0].recovery_attempts, 0);
    state::release_web_queue_item_worker_lease(&lease).unwrap();
}
