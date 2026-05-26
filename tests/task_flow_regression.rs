#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "qcold integration tests validate orchestration task-flow behavior rather than a documented public API"
)]

//! Regression coverage for the qcold-owned managed task-flow contract.

#[path = "task_flow_regression/fixture.rs"]
mod fixture;
#[path = "task_flow_regression/helpers.rs"]
mod helpers;
#[path = "support/task_flow_helpers.rs"]
mod task_flow_helpers;

use std::fs;
use std::path::Path;

use predicates::str::contains;

use fixture::{Fixture, BASE_BRANCH};
use helpers::{path_from_stdout, stdout_text};
use task_flow_helpers::{
    bundle_extract, bundle_extract_env, bundle_listing, git_output, load_task_env,
    terminal_receipt_relative_path, write_file,
};

#[test]
fn task_open_uses_generated_xtask_fixture_and_creates_managed_worktree() {
    let fixture = Fixture::new();

    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "self-contained"])
        .assert()
        .success()
        .stdout(contains("task-opened\tself-contained"))
        .stdout(contains("TASK_WORKTREE="));
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    assert!(worktree.join(".task/task.env").is_file());

    let task = load_task_env(&worktree);
    assert_eq!(task.task_name, "self-contained");
    assert_eq!(task.task_branch, "task/self-contained");
    assert_eq!(task.primary_repo_path, fixture.primary);
    assert_eq!(task.task_profile, "e2e");
    assert_eq!(task.status.as_str(), "open");
}

#[test]
fn task_open_preserves_explicit_slim_profile() {
    let fixture = Fixture::new();

    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "slim-work", "slim"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");

    assert_eq!(load_task_env(&worktree).task_profile, "slim");
}

#[test]
fn task_pause_then_open_resumes_existing_worktree() {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "resume"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");

    fixture
        .run_xtask(&worktree, &["task", "pause", "--reason", "operator wait"])
        .assert()
        .success()
        .stdout(contains("task-pause\tresume"));

    let reopened = fixture
        .run_xtask(&fixture.primary, &["task", "open", "resume"])
        .assert()
        .success()
        .stdout(contains("task-resumed\tresume"));
    assert_eq!(
        path_from_stdout(&stdout_text(&reopened), "TASK_WORKTREE"),
        worktree
    );
    assert_eq!(load_task_env(&worktree).status.as_str(), "open");
}

#[test]
fn current_bundle_command_creates_source_bundle() {
    let fixture = Fixture::new();

    let bundle = fixture
        .run_xtask(&fixture.primary, &["task", "bundle"])
        .assert()
        .success()
        .stdout(contains("BUNDLE_PATH="));
    let bundle_path = path_from_stdout(&stdout_text(&bundle), "BUNDLE_PATH");
    assert!(bundle_path.is_file());
    assert!(
        bundle_listing(&bundle_path).contains("repo/file.txt")
            || bundle_listing(&bundle_path).contains("file.txt")
    );
}

#[test]
fn blocked_and_failed_closeout_create_terminal_receipt_bundles_and_cleanup() {
    let fixture = Fixture::new();

    let blocked_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "blocked"])
        .assert()
        .success();
    let blocked_worktree = path_from_stdout(&stdout_text(&blocked_open), "TASK_WORKTREE");
    write_file(&blocked_worktree.join("note.txt"), "blocked\n");
    let blocked = fixture
        .run_xtask(
            &blocked_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "blocked",
                "--reason",
                "resume later",
            ],
        )
        .assert()
        .code(10)
        .stdout(contains("BUNDLE_PATH="));
    assert_terminal_receipt(
        &path_from_stdout(&stdout_text(&blocked), "BUNDLE_PATH"),
        "blocked",
        "operator_blocked",
    );
    assert!(!blocked_worktree.exists());

    let failed_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "failed"])
        .assert()
        .success();
    let failed_worktree = path_from_stdout(&stdout_text(&failed_open), "TASK_WORKTREE");
    write_file(&failed_worktree.join("failed.txt"), "failed\n");
    let failed = fixture
        .run_xtask(
            &failed_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "failed",
                "--reason",
                "simulated failure",
            ],
        )
        .assert()
        .code(11)
        .stdout(contains("BUNDLE_PATH="));
    assert_terminal_receipt(
        &path_from_stdout(&stdout_text(&failed), "BUNDLE_PATH"),
        "failed",
        "operator_failed",
    );
    assert!(!failed_worktree.exists());
}

#[test]
fn success_closeout_delivers_task_branch_to_primary_and_pushes_base() {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "ship"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(&worktree.join("ship.txt"), "ship\n");

    let closeout = fixture
        .run_xtask(
            &worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "docs: close self-hosted task",
            ],
        )
        .assert()
        .success()
        .stdout(contains("task-closeout\tsuccess\tship"));
    let stdout = stdout_text(&closeout);
    assert!(stdout.contains("task-closeout-phase\tstart\tpre-merge-review"));
    assert!(stdout.contains("task-closeout-review\tstarted\t"));
    assert!(stdout.contains("task-closeout-review\tfinished\texit_status=0"));
    assert!(stdout.contains("task-closeout-phase\tok\tpre-merge-review"));
    assert!(stdout.contains("PRE_MERGE_REVIEW_SUMMARY=Fixture review passed."));
    let bundle = path_from_stdout(&stdout, "BUNDLE_PATH");
    let listing = bundle_listing(&bundle);
    assert!(listing.contains("metadata/pre-merge-review.env"));
    assert!(listing.contains("evidence/pre-merge-review.md"));
    let review_env = bundle_extract_env(&bundle, "metadata/pre-merge-review.env");
    assert_eq!(
        review_env.get("PRE_MERGE_REVIEW_STATUS"),
        Some(&"pass".to_string())
    );
    assert_eq!(
        review_env.get("PRE_MERGE_REVIEW_SUMMARY"),
        Some(&"Fixture review passed.".to_string())
    );
    let review_report = bundle_extract(&bundle, "evidence/pre-merge-review.md");
    assert!(review_report.contains("REVIEW_STATUS=pass"));
    let summary = bundle_extract(&bundle, "summary.md");
    assert!(summary.contains("- Pre-merge review: `pass`"));
    assert!(summary.contains("- Review summary: Fixture review passed."));

    assert!(!worktree.exists());
    assert_eq!(git_output(&fixture.primary, &["status", "--porcelain"]), "");
    let primary_head = git_output(&fixture.primary, &["rev-parse", BASE_BRANCH]);
    let origin_head = git_output(
        &fixture.primary,
        &["rev-parse", &format!("origin/{BASE_BRANCH}")],
    );
    assert_eq!(primary_head, origin_head);
    assert!(fixture.primary.join("ship.txt").is_file());
    assert!(!fixture.primary.join(".task").exists());
    assert_eq!(git_output(&fixture.primary, &["ls-files", ".task"]), "");
}

#[test]
fn success_closeout_reviewer_block_preserves_task_before_delivery() {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "review-block"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(&worktree.join("blocked.txt"), "blocked\n");

    let mut closeout = fixture.run_xtask(
        &worktree,
        &[
            "task",
            "closeout",
            "--outcome",
            "success",
            "--message",
            "docs: blocked by reviewer",
        ],
    );
    closeout.env(
        "QCOLD_CLOSEOUT_REVIEWER_COMMAND",
        "cat > \"$QCOLD_REVIEW_OUTPUT\" <<'QCOLD_REVIEW'\n\
         REVIEW_STATUS=block\n\
         REVIEW_SUMMARY=Architecture issue.\n\
         - Architecture issue is blocking because it bypasses the adapter boundary.\n\
         QCOLD_REVIEW",
    );
    let closeout = closeout
        .assert()
        .code(12)
        .stdout(contains("task-closeout\tfailed-closeout\treview-block"))
        .stdout(contains("CLOSEOUT_FAILURE_PHASE=pre-merge-review"))
        .stdout(contains("BUNDLE_PATH="));

    assert!(worktree.exists());
    assert!(!fixture.primary.join("blocked.txt").exists());
    assert_eq!(load_task_env(&worktree).status.as_str(), "failed-closeout");
    let bundle = path_from_stdout(&stdout_text(&closeout), "BUNDLE_PATH");
    let review_env = bundle_extract_env(&bundle, "metadata/pre-merge-review.env");
    assert_eq!(
        review_env.get("PRE_MERGE_REVIEW_STATUS"),
        Some(&"block".to_string())
    );
    let receipt = bundle_extract_env(&bundle, terminal_receipt_relative_path());
    assert_eq!(
        receipt.get("CLOSEOUT_FAILURE_PHASE"),
        Some(&"pre-merge-review".to_string())
    );
}

#[test]
fn success_closeout_reviewer_command_failure_bundles_diagnostics() {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "review-error"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(&worktree.join("error.txt"), "error\n");

    let mut closeout = fixture.run_xtask(
        &worktree,
        &[
            "task",
            "closeout",
            "--outcome",
            "success",
            "--message",
            "docs: reviewer command fails",
        ],
    );
    closeout.env(
        "QCOLD_CLOSEOUT_REVIEWER_COMMAND",
        "printf 'reviewer exploded\\n' >&2; exit 42",
    );
    let closeout = closeout
        .assert()
        .code(12)
        .stdout(contains("task-closeout\tfailed-closeout\treview-error"))
        .stdout(contains("CLOSEOUT_FAILURE_PHASE=pre-merge-review"))
        .stdout(contains("BUNDLE_PATH="));

    let bundle = path_from_stdout(&stdout_text(&closeout), "BUNDLE_PATH");
    let listing = bundle_listing(&bundle);
    assert!(listing.contains("evidence/pre-merge-review-prompt.md"));
    assert!(listing.contains("evidence/pre-merge-review-command.log"));
    let command_log = bundle_extract(&bundle, "evidence/pre-merge-review-command.log");
    assert!(command_log.contains("reviewer exploded"));
    assert!(worktree.exists());
    assert!(!fixture.primary.join("error.txt").exists());
}

#[test]
fn success_closeout_failure_records_failed_closeout_bundle_without_cleanup() {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "broken-success"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    write_file(&worktree.join("payload.txt"), "payload\n");
    fs::create_dir_all(worktree.join("target")).unwrap();
    write_file(&worktree.join("target/build-output.txt"), "generated\n");
    corrupt_base_branch(&worktree);

    let closeout = fixture
        .run_xtask(
            &worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "docs: should fail",
            ],
        )
        .assert()
        .code(12)
        .stdout(contains("task-closeout\tfailed-closeout\tbroken-success"))
        .stdout(contains("CLOSEOUT_FAILURE_PHASE=deliver-to-primary"))
        .stdout(contains("BUNDLE_PATH="));

    assert!(worktree.exists());
    assert_eq!(load_task_env(&worktree).status.as_str(), "failed-closeout");
    let stdout = stdout_text(&closeout);
    let bundle = path_from_stdout(&stdout, "BUNDLE_PATH");
    assert_terminal_receipt(&bundle, "failed-closeout", "success_closeout_failed");
    let listing = bundle_listing(&bundle);
    assert!(listing.contains("repo/payload.txt"));
    assert!(!listing.contains("target/build-output.txt"));
    let receipt = bundle_extract_env(&bundle, terminal_receipt_relative_path());
    assert_eq!(
        receipt.get("CURRENT_FLOW_PROBLEM"),
        Some(&"success_closeout_failed".to_string())
    );
    assert_eq!(
        receipt.get("CLOSEOUT_FAILURE_PHASE"),
        Some(&"deliver-to-primary".to_string())
    );
    assert!(receipt
        .get("CLOSEOUT_FAILURE_ERROR")
        .is_some_and(|value| value.contains("deliver-to-primary")));
}

#[test]
fn terminal_check_reports_open_task_state() {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "pending"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");

    fixture
        .run_xtask(&fixture.primary, &["task", "terminal-check"])
        .assert()
        .code(1)
        .stdout(contains(format!(
            "open-task\tpending\t{}",
            worktree.display()
        )))
        .stderr(contains("terminal-check blocked"));
}

#[test]
fn verify_preflight_runs_directly_inside_container_runtime() {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "container-runtime"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");

    fixture
        .run_qcold_in_container_runtime(&worktree, &["verify", "fast"])
        .assert()
        .success();
    assert!(!fixture.devcontainer_log_text().contains("exec|"));
}

fn assert_terminal_receipt(bundle: &Path, outcome: &str, category: &str) {
    assert!(bundle.is_file(), "missing bundle {}", bundle.display());
    let listing = bundle_listing(bundle);
    assert!(listing.contains("summary.md"));
    assert!(listing.contains("metadata/bundle.env"));
    assert!(listing.contains(terminal_receipt_relative_path()));
    let receipt = bundle_extract_env(bundle, terminal_receipt_relative_path());
    assert_eq!(receipt.get("OUTCOME"), Some(&outcome.to_string()));
    assert_eq!(
        receipt.get("CLOSEOUT_CATEGORY"),
        Some(&category.to_string())
    );
    let summary = bundle_extract(bundle, "summary.md");
    assert!(summary.contains(&format!("- Outcome: `{outcome}`")));
    assert!(summary.contains(&format!("- Closeout category: `{category}`")));
    assert!(summary.contains("metadata/terminal-receipt.env"));
}

fn corrupt_base_branch(worktree: &Path) {
    let task_env_path = worktree.join(".task/task.env");
    let task_env = fs::read_to_string(&task_env_path).unwrap();
    fs::write(
        &task_env_path,
        task_env.replace(
            &format!("BASE_BRANCH={BASE_BRANCH}"),
            "BASE_BRANCH='does-not-exist'",
        ),
    )
    .unwrap();
}
