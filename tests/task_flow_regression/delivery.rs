use super::*;

pub(crate) fn success_closeout_delivers_directly_and_records_delivery_metadata() {
    let fixture = Fixture::new();

    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "direct-happy"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(&worktree.join("direct.txt"), "direct delivery\n");

    let closeout = fixture
        .run_xtask(
            &worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "docs: close direct path",
            ],
        )
        .assert()
        .success();
    let closeout_stdout = stdout_text(&closeout);
    let bundle = path_from_stdout(&closeout_stdout, "BUNDLE_PATH");
    let receipt = bundle_extract_env(&bundle, terminal_receipt_relative_path());
    let bundle_env = bundle_extract_env(&bundle, "metadata/bundle.env");
    assert_eq!(receipt.get("DELIVERY_MODE"), Some(&"direct".to_string()));
    assert_eq!(receipt.get("REVIEW_ID"), Some(&String::new()));
    assert_eq!(receipt.get("REVIEW_URL"), Some(&String::new()));
    assert_eq!(receipt.get("DELIVERED_HEAD"), bundle_env.get("TASK_HEAD"));
    assert!(receipt
        .get("MERGED_HEAD")
        .is_some_and(|value| !value.trim().is_empty()));

    let merged_head = git_output(&fixture.primary, &["rev-parse", BASE_BRANCH]);
    assert_eq!(receipt.get("MERGED_HEAD"), Some(&merged_head));
    let parent_line = git_output(
        &fixture.primary,
        &["rev-list", "--parents", "-n", "1", BASE_BRANCH],
    );
    assert_eq!(parent_line.split_whitespace().count(), 2);
    let remote_branch = Command::new("git")
        .current_dir(&fixture.primary)
        .args(["ls-remote", "--heads", "origin", "task/direct-happy"])
        .output()
        .unwrap();
    assert!(remote_branch.status.success());
    assert!(String::from_utf8(remote_branch.stdout)
        .unwrap()
        .trim()
        .is_empty());
}

pub(crate) fn success_closeout_treats_legacy_merge_request_mode_as_direct() {
    let fixture = Fixture::new();
    git(
        &fixture.primary,
        &["config", "taskflow.success-delivery", "merge-request"],
    );

    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "legacy-direct"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(
        &worktree.join("legacy.txt"),
        "legacy config direct delivery\n",
    );

    let closeout = fixture
        .run_xtask(
            &worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "docs: close legacy direct path",
            ],
        )
        .assert()
        .success();
    let closeout_stdout = stdout_text(&closeout);
    let bundle = path_from_stdout(&closeout_stdout, "BUNDLE_PATH");
    let receipt = bundle_extract_env(&bundle, terminal_receipt_relative_path());
    assert_eq!(receipt.get("DELIVERY_MODE"), Some(&"direct".to_string()));
    assert_eq!(receipt.get("REVIEW_ID"), Some(&String::new()));
    assert_eq!(receipt.get("REVIEW_URL"), Some(&String::new()));
}
