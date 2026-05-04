use super::*;

pub(crate) fn iteration_notify_sends_non_terminal_handoff_message_and_preserves_task_state() {
    let fixture = Fixture::new();

    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "iteration"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(&worktree.join("note.txt"), "still in progress\n");

    let _closeout = fixture
        .run_xtask(
            &worktree,
            &[
                "task",
                "iteration-notify",
                "--message",
                "waiting for operator direction",
            ],
        )
        .assert()
        .success()
        .stdout(contains("ITERATION_NOTIFICATION=sent"))
        .stdout(contains("ITERATION_STATE=task-open"));

    assert!(worktree.exists());
    let telegram_request: Value =
        serde_json::from_str(&fixture.last_telegram_request().unwrap()).unwrap();
    let telegram_text = telegram_request["text"].as_str().unwrap();
    assert!(telegram_text.contains("*ITERATION*"));
    let open_stdout = stdout_text(&open);
    let iteration_anchor = parse_value("TASK_EXECUTION_ANCHOR", &open_stdout).unwrap();
    assert!(telegram_text.contains(&format!("*Anchor:* `{}`", iteration_anchor)));
    assert!(telegram_text.contains("*Update:* waiting for operator direction"));
    assert!(telegram_text.contains("*Branch:* `task/iteration`"));
}

pub(crate) fn verify_preflight_runs_directly_inside_container_runtime() {
    let fixture = Fixture::new();

    let open = fixture
        .run_xtask(
            &fixture.primary,
            &["task", "open", "container-runtime-closeout"],
        )
        .assert()
        .success();
    let worktree = task_worktree_from_assert(&open);
    write_file(
        &worktree.join("container-closeout.txt"),
        "payload
",
    );
    git(&worktree, &["add", "container-closeout.txt"]);
    fixture.clear_devcontainer_log();

    fixture
        .run_qcold_in_container_runtime(&worktree, &["verify", "preflight"])
        .assert()
        .success()
        .stdout(contains("[verify-preflight] task metadata status=ok"))
        .stdout(contains("[verify-preflight] ok"));
    let devcontainer_log = fixture.devcontainer_log_text();
    assert!(!devcontainer_log.contains("exec|"));
}
