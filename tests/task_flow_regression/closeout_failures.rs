use super::*;

pub(crate) fn markdown_only_success_closeout_skips_canonical_validation() {
    let fixture = Fixture::new();

    let success_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "docs-only"])
        .assert()
        .success();
    let success_worktree = path_from_stdout(&stdout_text(&success_open), "TASK_WORKTREE");
    write_file(&success_worktree.join("docs-only.md"), "docs only\n");

    let success = fixture
        .run_xtask(
            &success_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "docs: close markdown-only path",
            ],
        )
        .assert()
        .success();
    let success_stdout = stdout_text(&success);
    let success_bundle = path_from_stdout(&success_stdout, "BUNDLE_PATH");
    assert!(success_bundle.exists());

    let validation_log = fs::read_to_string(&fixture.validation_log).unwrap_or_default();
    assert!(!validation_log.contains("verify-autofix|"));
    assert!(!validation_log.contains("verify-preflight|"));
    assert!(!validation_log.contains("verify-fast|"));

    let receipt = bundle_extract_env(&success_bundle, terminal_receipt_relative_path());
    assert_eq!(
        receipt.get("CANONICAL_VALIDATION").unwrap(),
        "markdown-only diff: canonical verify skipped"
    );
}

pub(crate) fn success_closeout_omits_repository_terminal_state_when_other_tasks_remain_open() {
    let fixture = Fixture::new();

    let lingering_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "linger"])
        .assert()
        .success();
    let lingering_worktree = path_from_stdout(&stdout_text(&lingering_open), "TASK_WORKTREE");
    write_file(&lingering_worktree.join("linger-pending.txt"), "pending\n");

    let shipping_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "ship"])
        .assert()
        .success();
    let shipping_worktree = path_from_stdout(&stdout_text(&shipping_open), "TASK_WORKTREE");
    write_file(&shipping_worktree.join("ship.txt"), "ship\n");

    let closeout = fixture
        .run_xtask(
            &shipping_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "docs: close one task while another stays open",
            ],
        )
        .assert()
        .success();
    let stdout = stdout_text(&closeout);
    assert!(!stdout.contains("TERMINAL_STATE="));
    assert!(!stdout.contains("OPEN_TASK="));
    assert!(!stdout.contains("INTERRUPTED_TASK="));
    assert!(!stdout.contains("TERMINAL_PENDING="));
    assert!(!stdout.contains("TERMINAL_CHECK="));
    assert!(!shipping_worktree.exists());
    assert!(lingering_worktree.exists());

    fixture
        .run_xtask(&fixture.primary, &["task", "terminal-check"])
        .assert()
        .code(1)
        .stdout(contains(format!(
            "open-task\tlinger\t{}",
            lingering_worktree.display()
        )))
        .stderr(contains(
            "terminal-check blocked: managed task worktrees remain open",
        ));

    write_file(&lingering_worktree.join("linger.txt"), "linger\n");
    fixture
        .run_xtask(
            &lingering_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "blocked",
                "--reason",
                "linger cleanup",
            ],
        )
        .assert()
        .code(10);

    fixture
        .run_xtask(&fixture.primary, &["task", "terminal-check"])
        .assert()
        .success();
}

pub(crate) fn incomplete_success_closeout_emits_failed_bundle_and_preserves_worktree() {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "incomplete"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(&worktree.join("incomplete.txt"), "incomplete\n");
    git(&worktree, &["add", "incomplete.txt"]);
    git(&worktree, &["commit", "-m", "incomplete fixture"]);

    let task_env_path = worktree.join(".task/task.env");
    let task_env = fs::read_to_string(&task_env_path).unwrap();
    fs::write(
        &task_env_path,
        task_env.replace(
            &format!("BASE_BRANCH='{BASE_BRANCH}'"),
            "BASE_BRANCH='does-not-exist'",
        ),
    )
    .unwrap();

    let closeout = fixture
        .run_xtask(
            &worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "should fail",
            ],
        )
        .assert()
        .code(1)
        .stderr(contains("fallback_bundle="));
    let stderr = String::from_utf8(closeout.get_output().stderr.clone()).unwrap();
    assert!(!stderr.contains("run 'cargo xtask bundle'"));
    assert!(stderr.contains("that bundle is the evidence bundle"));
    let stdout = stdout_text(&closeout);
    let bundle = path_from_stdout(&stdout, "BUNDLE_PATH");
    assert!(bundle.exists());
    assert!(bundle
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("failed-closeout"));
    assert!(bundle_listing(&bundle).contains("logs/events.ndjson"));
    assert!(!bundle_listing(&bundle).contains(terminal_receipt_relative_path()));
    assert!(worktree.exists());
    assert_eq!(fixture.telegram_request_count(), 0);

    let reopened = fixture
        .run_xtask(&fixture.primary, &["task", "open", "incomplete"])
        .assert()
        .success()
        .stdout(contains("[task-open] worktree action=resume"));
    let reopened_stdout = stdout_text(&reopened);
    assert_eq!(
        path_from_stdout(&reopened_stdout, "TASK_WORKTREE"),
        worktree
    );
    let mut reopened_env = load_task_env(&worktree);
    assert_eq!(reopened_env.status, TaskStatus::Open);
    assert!(reopened_env.last_bundle.is_empty());

    reopened_env.base_branch = "does-not-exist".into();
    save_task_env(&worktree, &reopened_env);

    let reopened_closeout = fixture
        .run_xtask(
            &worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "should fail again",
            ],
        )
        .assert()
        .code(1)
        .stderr(contains("fallback_bundle="));
    let reopened_closeout_stdout = stdout_text(&reopened_closeout);
    let reopened_bundle = path_from_stdout(&reopened_closeout_stdout, "BUNDLE_PATH");
    assert!(reopened_bundle.exists());
    assert_ne!(reopened_bundle, bundle);
}

#[cfg(unix)]
pub(crate) fn post_delivery_cleanup_failure_is_logged_without_blocking_success_closeout() {
    let fixture = Fixture::new();
    let orphan_root = fixture.temp.path().join("WT").join("primary");
    let orphan = orphan_root.join("stale-orphan");
    fs::create_dir_all(&orphan).unwrap();
    let orphan_file = orphan.join("payload.txt");
    fs::write(&orphan_file, "stale\n").unwrap();
    for path in [&orphan, &orphan_file] {
        let status = Command::new("touch")
            .args(["-t", "200001010000"])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success(), "touch failed for {}", path.display());
    }

    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "cleanup-notify"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(&worktree.join("cleanup.txt"), "cleanup\n");

    let before_requests = fixture.telegram_request_count();
    let closeout = fixture
        .run_xtask(
            &worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "docs: survive post-delivery cleanup failure",
            ],
        )
        .env(
            "QCOLD_TASKFLOW_TEST_FAIL_STALE_ORPHAN_CLEAR",
            orphan.display().to_string(),
        )
        .assert()
        .success()
        .stderr(contains(
            "[task-closeout] auto-clear-stale-orphans status=failed",
        ));
    let closeout_stdout = stdout_text(&closeout);
    let bundle = path_from_stdout(&closeout_stdout, "BUNDLE_PATH");
    assert!(bundle.exists());
    assert!(!worktree.exists());
    assert!(fixture.telegram_request_count() > before_requests);
    let telegram_request: Value =
        serde_json::from_str(&fixture.last_telegram_request().unwrap()).unwrap();
    let telegram_text = telegram_request["text"].as_str().unwrap();
    assert!(telegram_text.contains("*SUCCESS*"));
    assert!(!telegram_text.contains("terminal closeout cleanup failed"));
}

pub(crate) fn success_closeout_fails_before_validation_when_primary_dirty_overlaps_open_task() {
    let fixture = Fixture::new();

    let owner_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "owner"])
        .assert()
        .success();
    let owner_worktree = path_from_stdout(&stdout_text(&owner_open), "TASK_WORKTREE");

    let shipping_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "ship-dirty-primary"])
        .assert()
        .success();
    let shipping_worktree = path_from_stdout(&stdout_text(&shipping_open), "TASK_WORKTREE");
    write_file(&owner_worktree.join("shared.txt"), "owner task dirt\n");
    write_file(&fixture.primary.join("shared.txt"), "primary dirt\n");
    write_file(&shipping_worktree.join("ship.txt"), "ship\n");

    let closeout = fixture
        .run_xtask(
            &shipping_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "docs: should stop on dirty primary overlap",
            ],
        )
        .assert()
        .code(1)
        .stderr(contains("[task-closeout] primary-dirty-file\tshared.txt"))
        .stderr(contains(format!(
            "[task-closeout] open-task-dirty-overlap\towner\tshared.txt\t{}",
            owner_worktree.display()
        )))
        .stderr(contains("primary checkout is dirty:"));
    let stdout = stdout_text(&closeout);
    let bundle = path_from_stdout(&stdout, "BUNDLE_PATH");
    assert!(bundle.exists());
    assert!(shipping_worktree.exists());
    assert!(owner_worktree.exists());
    let validation_log = fs::read_to_string(&fixture.validation_log).unwrap_or_default();
    assert!(!validation_log.contains("verify-autofix|"));
    assert!(!validation_log.contains("verify-preflight|"));
    assert!(!validation_log.contains("verify-fast|"));
    assert_eq!(fixture.telegram_request_count(), 0);
}

pub(crate) fn success_closeout_allows_failed_closeout_task_residue_and_leaves_terminal_check_non_terminal(
) {
    let fixture = Fixture::new();

    let tail_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "tail"])
        .assert()
        .success();
    let tail_worktree = path_from_stdout(&stdout_text(&tail_open), "TASK_WORKTREE");
    let mut tail_env = load_task_env(&tail_worktree);
    tail_env.status = "failed-closeout".into();
    save_task_env(&tail_worktree, &tail_env);

    let shipping_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "ship-tail-guard"])
        .assert()
        .success();
    let shipping_worktree = path_from_stdout(&stdout_text(&shipping_open), "TASK_WORKTREE");
    write_file(&shipping_worktree.join("ship.txt"), "ship\n");

    let closeout = fixture
        .run_xtask(
            &shipping_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "docs: close independent task despite preserved residue",
            ],
        )
        .assert()
        .success();
    let stdout = stdout_text(&closeout);
    let bundle = path_from_stdout(&stdout, "BUNDLE_PATH");
    assert!(bundle.exists());
    assert!(!stdout.contains("TERMINAL_STATE="));
    assert!(!stdout.contains("TERMINAL_CHECK="));
    assert!(!stdout.contains("TERMINAL_PENDING="));
    assert!(!shipping_worktree.exists());
    assert!(tail_worktree.exists());
    let validation_log = fs::read_to_string(&fixture.validation_log).unwrap();
    assert!(validation_log.contains("verify-autofix|"));
    assert!(validation_log.contains("verify-preflight|"));
    assert!(validation_log.contains("verify-fast|"));

    fixture
        .run_xtask(&fixture.primary, &["task", "terminal-check"])
        .assert()
        .code(1)
        .stdout(contains(format!(
            "open-task\ttail\t{}",
            tail_worktree.display()
        )))
        .stdout(contains(format!(
            "incomplete-task\ttail\tfailed-closeout\t{}",
            tail_worktree.display()
        )))
        .stderr(contains(
            "terminal-check blocked: incomplete failed-closeout task state remains",
        ));
}

pub(crate) fn cleanup_failure_scrubs_terminal_receipt_from_incomplete_bundle() {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "cleanup-failure"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(
        &worktree.join("cleanup.txt"),
        "cleanup failure
",
    );

    let original_docker = fixture.temp.path().join("docker-ok");
    fs::copy(fixture.fakebin.join("docker"), &original_docker).unwrap();
    write_exe(
        &fixture.fakebin.join("docker"),
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "${{1:-}}" == "rm" ]]; then
  printf 'simulated docker rm failure
' >&2
  exit 1
fi
exec "{docker}" "$@"
"#,
            docker = original_docker.display(),
        ),
    );

    let closeout = fixture
        .run_xtask(
            &worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "blocked",
                "--reason",
                "cleanup failure",
            ],
        )
        .assert()
        .code(1)
        .stderr(contains("preserved_bundle="))
        .stderr(contains("cleanup=skipped"));
    let stdout = stdout_text(&closeout);
    let bundle = path_from_stdout(&stdout, "BUNDLE_PATH");
    assert!(bundle.exists());
    assert!(!bundle_listing(&bundle).contains(terminal_receipt_relative_path()));
    assert!(worktree.exists());
    assert!(Command::new("git")
        .current_dir(&worktree)
        .args(["status", "--short"])
        .status()
        .unwrap()
        .success());
}

pub(crate) fn late_closeout_failure_preserves_precleanup_bundle_and_keeps_git_worktree_valid() {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "late-failure"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(&worktree.join("late.txt"), "late\n");
    git(&worktree, &["add", "late.txt"]);
    git(&worktree, &["commit", "-m", "late failure fixture"]);

    let zip_probe = fixture.temp.path().join("zip-count");
    write_exe(
        &fixture.fakebin.join("7z"),
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
if [[ "${{1:-}}" == "a" ]]; then
  count_file={count_file:?}
  count=0
  if [[ -f "$count_file" ]]; then
    count=$(cat "$count_file")
  fi
  count=$((count+1))
  printf '%s' "$count" >"$count_file"
  if [[ "$count" -eq 2 ]]; then
    printf 'simulated 7z failure on second deterministic archive invocation\n' >&2
    exit 1
  fi
fi
exec /usr/bin/7z "$@"
"#,
            count_file = zip_probe.display(),
        ),
    );

    let closeout = fixture
        .run_xtask(
            &worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "blocked",
                "--reason",
                "late zip failure",
            ],
        )
        .assert()
        .code(1)
        .stderr(contains("preserved_bundle="))
        .stderr(contains("cleanup=skipped"));
    let stdout = stdout_text(&closeout);
    let stderr = String::from_utf8(closeout.get_output().stderr.clone()).unwrap();
    let bundle = path_from_stdout(&stdout, "BUNDLE_PATH");
    assert!(bundle.exists());
    assert!(bundle_listing(&bundle).contains("metadata/bundle.env"));
    assert!(!bundle_listing(&bundle).contains(terminal_receipt_relative_path()));
    assert!(worktree.exists());
    assert!(Command::new("git")
        .current_dir(&worktree)
        .args(["status", "--short"])
        .status()
        .unwrap()
        .success());
    assert!(stderr.contains("task state preserved for investigation"));
    assert_eq!(fixture.telegram_request_count(), 0);
}
