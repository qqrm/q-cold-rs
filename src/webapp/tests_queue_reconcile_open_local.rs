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
    assert_eq!(
        stored_items
            .iter()
            .map(|item| (item.id.as_str(), item.status.as_str(), item.message.as_str()))
            .collect::<Vec<_>>(),
        [
            ("first", "stopped", LOCAL_OPEN_RECORD_STOPPED_MESSAGE),
            ("second", "pending", "")
        ]
    );
    assert!(test_web_queue_worker_spawned(&run.id));
}

#[test]
fn local_open_task_record_without_live_agent_stops_active_item() {
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

    assert!(matches!(outcome, QueueItemOutcome::Stopped));
    assert_eq!(stored_run.status, "stopped");
    assert_eq!(
        stored_items
            .iter()
            .map(|item| (item.id.as_str(), item.status.as_str(), item.message.as_str()))
            .collect::<Vec<_>>(),
        [("first", "stopped", LOCAL_OPEN_RECORD_STOPPED_MESSAGE)]
    );
}

#[test]
fn stopped_local_open_task_record_stays_ready_for_resume_worker() {
    let _guard = test_support::env_guard();
    let temp = tempdir().unwrap();
    std::env::set_var("QCOLD_STATE_DIR", temp.path());
    let run = queue_run_fixture("stopped-local-open-task-record", "running", 0);
    let item = queue_item_fixture(&run.id, "first", 0, "stopped", Some("qa-task-first"));
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
        QueueReconcile::Unchanged
    ));
    let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
    let stored_run = stored_run.unwrap();

    assert_eq!(stored_run.status, "running");
    assert_eq!(
        stored_items
            .iter()
            .map(|item| (item.id.as_str(), item.status.as_str()))
            .collect::<Vec<_>>(),
        [("first", "stopped")]
    );
    assert_eq!(queue_ready_item_ids(&stored_run, &stored_items), ids(&["first"]));
}
