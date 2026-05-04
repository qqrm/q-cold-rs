#![allow(
    missing_docs,
    clippy::cast_possible_truncation,
    clippy::cognitive_complexity,
    clippy::expect_used,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::used_underscore_binding,
    clippy::unwrap_used,
    reason = "qcold integration tests validate orchestration control-plane behavior rather than a documented public API"
)]

//! Integration coverage for qcold-owned task-flow control-plane entrypoints.

#[path = "support/task_flow_helpers.rs"]
mod task_flow_helpers;

use assert_cmd::Command as AssertCommand;
use predicates::str::contains;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::{tempdir, TempDir};

use task_flow_helpers::{
    bundle_extract_env, git, git_output, managed_root, parse_value, save_task_env,
    seed_required_control_plane_files, terminal_receipt_relative_path, write_exe, write_file,
    xtask_process_manifest, TaskEnv,
};

const BASE_BRANCH: &str = "developer";

fn git_status_success(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .unwrap()
        .success()
}

fn set_task_status(worktree: &Path, status: &str) {
    let mut env = task_flow_helpers::load_task_env(worktree);
    env.status = status.into();
    save_task_env(worktree, &env);
}

fn run_qcold(repo: &Path, fakebin: &Path, args: &[&str]) -> AssertCommand {
    let original_path = env::var("PATH").unwrap_or_default();
    let notification_env_file = repo.join(".taskflow-test-notify.env");
    let mut cmd = AssertCommand::cargo_bin("cargo-qcold").unwrap();
    cmd.current_dir(repo)
        .args(args)
        .env("PATH", format!("{}:{original_path}", fakebin.display()))
        .env("QCOLD_REPO_ROOT", repo)
        .env("QCOLD_XTASK_MANIFEST", xtask_process_manifest())
        // Scrub any live managed-task markers so fixture repos cannot inherit
        // the caller's active task context or notification mounts.
        .env_remove("QCOLD_TASKFLOW_CONTAINER_ROOT")
        .env_remove("QCOLD_TASKFLOW_CONTEXT")
        .env_remove("QCOLD_TASKFLOW_PRIMARY_REPO_PATH")
        .env_remove("QCOLD_TASKFLOW_TASK_ID")
        .env_remove("QCOLD_TASKFLOW_TASK_WORKTREE")
        .env_remove("QCOLD_TASKFLOW_TASK_BRANCH")
        .env_remove("QCOLD_TASKFLOW_DEVCONTAINER_ID")
        .env_remove("QCOLD_DEVCONTAINER_PROFILE")
        .env_remove("QCOLD_TASKFLOW_AGENT_RUNNER")
        .env_remove("QCOLD_TASKFLOW_AGENT_ID")
        .env_remove("QCOLD_TASKFLOW_AGENT_MODEL")
        .env_remove("QCOLD_TASKFLOW_AGENT_STATUS")
        .env_remove("QCOLD_TASKFLOW_AGENT_REMAINING_CAPACITY")
        .env_remove("QCOLD_TASKFLOW_USAGE_INPUT_TOKENS")
        .env_remove("QCOLD_TASKFLOW_USAGE_CACHED_INPUT_TOKENS")
        .env_remove("QCOLD_TASKFLOW_USAGE_OUTPUT_TOKENS")
        .env_remove("QCOLD_TASKFLOW_USAGE_TOTAL_TOKENS")
        .env_remove("QCOLD_TASKFLOW_USAGE_CREDITS")
        .env("TELEGRAM_BOT_TOKEN", "")
        .env("TELEGRAM_CHAT_ID", "")
        .env_remove("TELEGRAM_API_BASE_URL")
        .env_remove("TELEGRAM_NOTIFY_TIMEOUT")
        .env("TELEGRAM_ENV_FILE", &notification_env_file)
        .env_remove("JIRA_URL")
        .env_remove("JIRA_PERSONAL_TOKEN")
        .env_remove("JIRA_PROJECT_KEY")
        .env_remove("JIRA_ISSUE_TYPE")
        .env_remove("JIRA_PARENT_KEY")
        .env_remove("JIRA_DONE_TRANSITION")
        .env_remove("JIRA_LABELS")
        .env("JIRA_ENV_FILE", &notification_env_file)
        .env_remove("JIRA_SYNC")
        .env_remove("JIRA_DEBUG_TO_TELEGRAM");
    cmd
}

fn run_qcold_in_managed_task_devcontainer(
    repo: &Path,
    primary: &Path,
    fakebin: &Path,
    args: &[&str],
) -> AssertCommand {
    let mut cmd = run_qcold(repo, fakebin, args);
    cmd.env("QCOLD_TASKFLOW_TEST_ASSUME_CONTAINER_RUNTIME", "1")
        .env("QCOLD_TASKFLOW_CONTEXT", "managed-task-devcontainer")
        .env("QCOLD_TASKFLOW_PRIMARY_REPO_PATH", primary)
        .env("QCOLD_TASKFLOW_TASK_WORKTREE", repo)
        .env("QCOLD_DEVCONTAINER_PROFILE", "fast");
    cmd
}

struct TaskRepoFixture {
    _temp: TempDir,
    primary: PathBuf,
    fakebin: PathBuf,
}

impl TaskRepoFixture {
    fn new() -> Self {
        let temp = tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        let primary = temp.path().join("primary");
        let fakebin = temp.path().join("fakebin");

        git_init_bare(&remote);
        git_clone(&remote, &primary);
        git(&primary, &["config", "user.name", "tester"]);
        git(&primary, &["config", "user.email", "tester@example.com"]);
        git(&primary, &["config", "taskflow.base-branch", BASE_BRANCH]);
        git(&primary, &["checkout", "-B", BASE_BRANCH]);
        seed_required_control_plane_files(&primary);
        write_file(&primary.join(".gitignore"), "bundles/\n");
        write_file(&primary.join("README.md"), "seed\n");
        git(&primary, &["add", "."]);
        git(&primary, &["commit", "-m", "seed"]);
        git(&primary, &["push", "-u", "origin", BASE_BRANCH]);
        let _ = Command::new("git")
            .current_dir(&primary)
            .args(["remote", "set-head", "origin", BASE_BRANCH])
            .status();

        fs::create_dir_all(&fakebin).unwrap();
        write_exe(
            &fakebin.join("docker"),
            "#!/usr/bin/env bash\nset -euo pipefail\ncase \"${1:-}\" in\n  ps|images|rm|rmi) exit 0 ;;\n  inspect) exit 1 ;;\n  *) exit 0 ;;\nesac\n",
        );

        Self {
            _temp: temp,
            primary,
            fakebin,
        }
    }

    fn create_task_worktree(&self, slug: &str) -> PathBuf {
        let managed = managed_root(&self.primary);
        fs::create_dir_all(&managed).unwrap();

        let branch = format!("task/{slug}");
        let worktree = managed.join(slug);
        git(
            &self.primary,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                worktree.to_str().unwrap(),
                BASE_BRANCH,
            ],
        );

        let task_head = git_output(&worktree, &["rev-parse", "HEAD"]);
        let task_env = TaskEnv {
            task_id: branch.clone(),
            task_name: slug.to_string(),
            task_branch: branch,
            task_execution_anchor: "purple-apple-042".to_string(),
            task_worktree: worktree.clone(),
            primary_repo_path: self.primary.clone(),
            base_branch: BASE_BRANCH.to_string(),
            task_head,
            started_at: "2026-04-13T00:00:00+0300".to_string(),
            status: "open".into(),
            updated_at: "2026-04-13T00:00:00+0300".to_string(),
            devcontainer_id: format!("{slug}-devcontainer"),
            ..TaskEnv::default()
        };
        save_task_env(&worktree, &task_env);
        write_file(
            &worktree.join(".task/logs/events.ndjson"),
            "{\"kind\":\"task-open\"}\n",
        );
        worktree
    }
}

fn git_init_bare(path: &Path) {
    let status = Command::new("git")
        .args(["init", "--bare", "--initial-branch=developer"])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "git init bare failed");
}

fn git_clone(remote: &Path, dest: &Path) {
    let status = Command::new("git")
        .args(["clone", remote.to_str().unwrap(), dest.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success(), "git clone failed");
}

fn create_submodule_remote(root: &Path, name: &str) -> PathBuf {
    let remote = root.join(format!("{name}.git"));
    let clone = root.join(format!("{name}-work"));
    git_init_bare(&remote);
    git_clone(&remote, &clone);
    git(&clone, &["config", "user.name", "tester"]);
    git(&clone, &["config", "user.email", "tester@example.com"]);
    write_file(&clone.join("README.md"), "submodule\n");
    git(&clone, &["add", "README.md"]);
    git(&clone, &["commit", "-m", "seed"]);
    git(&clone, &["push", "-u", "origin", BASE_BRANCH]);
    remote
}

#[test]
fn inspect_stays_in_primary_checkout_and_creates_no_task_state() {
    let fixture = TaskRepoFixture::new();
    let managed = managed_root(&fixture.primary);
    assert!(!managed.exists());

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "inspect", "runtime-audit"],
    )
    .assert()
    .success()
    .stdout(contains("[task-inspect] primary action=sync"))
    .stdout(contains("[task-inspect] ready path="))
    .stdout(contains("closeout=not-required"))
    .stdout(contains("[task-inspect] topic=runtime-audit"))
    .stdout(contains(
        "[task-inspect] mode=read-only no-worktree no-devcontainer",
    ));

    assert!(
        !managed.exists() || fs::read_dir(&managed).unwrap().next().is_none(),
        "task inspect unexpectedly created managed worktree residue under {}",
        managed.display()
    );
    assert!(!fixture.primary.join(".task/task.env").exists());
    assert!(!git_status_success(
        &fixture.primary,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/task/runtime-audit"
        ]
    ));
}

#[test]
fn terminal_check_reports_open_tasks_and_clear_restores_terminal_state() {
    let fixture = TaskRepoFixture::new();
    let worktree = fixture.create_task_worktree("control");

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "terminal-check"],
    )
    .assert()
    .code(1)
    .stdout(contains(format!(
        "open-task\tcontrol\t{}",
        worktree.display()
    )))
    .stderr(contains(
        "terminal-check blocked: managed task worktrees remain open",
    ));

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "clear", "control"],
    )
    .assert()
    .success()
    .stdout(contains("[task-clear] cleared task=task/control"));
    assert!(!worktree.exists());
    assert!(!git_status_success(
        &fixture.primary,
        &["show-ref", "--verify", "--quiet", "refs/heads/task/control"]
    ));

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "terminal-check"],
    )
    .assert()
    .success()
    .stdout(contains(format!(
        "terminal-ok\t{}\t{}",
        fixture.primary.display(),
        BASE_BRANCH
    )));
}

#[test]
fn terminal_check_flags_incomplete_failed_closeout_task_residue() {
    let fixture = TaskRepoFixture::new();
    let worktree = fixture.create_task_worktree("tail");
    set_task_status(&worktree, "failed-closeout");

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "terminal-check"],
    )
    .assert()
    .code(1)
    .stdout(contains(format!("open-task\ttail\t{}", worktree.display())))
    .stdout(contains(format!(
        "incomplete-task\ttail\tfailed-closeout\t{}",
        worktree.display()
    )))
    .stderr(contains("incomplete failed-closeout task state remains"));
}

#[test]
fn clean_refuses_dirty_task_worktrees_but_clear_recovers_terminal_state() {
    let fixture = TaskRepoFixture::new();
    let worktree = fixture.create_task_worktree("dirty");
    write_file(&worktree.join("dirty.txt"), "untracked\n");

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "clean", "dirty"],
    )
    .assert()
    .code(1)
    .stderr(contains("dirty task worktree"));
    assert!(worktree.exists());

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "clear", "dirty"],
    )
    .assert()
    .success();
    assert!(!worktree.exists());

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "terminal-check"],
    )
    .assert()
    .success();
}

#[test]
fn terminal_check_accepts_materialized_primary_submodules_when_git_status_is_clean() {
    let fixture = TaskRepoFixture::new();
    let submodule_remote = create_submodule_remote(fixture._temp.path(), "cpp-btree");

    git(
        &fixture.primary,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            submodule_remote.to_str().unwrap(),
            "cpp-btree",
        ],
    );
    git(&fixture.primary, &["add", ".gitmodules", "cpp-btree"]);
    git(&fixture.primary, &["commit", "-m", "add submodule"]);
    git(&fixture.primary, &["push", "origin", BASE_BRANCH]);

    assert_eq!(
        git_output(
            &fixture.primary,
            &["status", "--porcelain", "--untracked-files=all"]
        ),
        ""
    );

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "terminal-check"],
    )
    .assert()
    .success()
    .stdout(contains("terminal-ok\t"))
    .stdout(contains(BASE_BRANCH));
}

#[test]
fn terminal_check_reports_primary_dirty_overlap_with_open_task() {
    let fixture = TaskRepoFixture::new();
    let worktree = fixture.create_task_worktree("owner");
    write_file(&fixture.primary.join("shared.txt"), "primary dirty\n");
    write_file(&worktree.join("shared.txt"), "task dirty\n");

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "terminal-check"],
    )
    .assert()
    .code(1)
    .stdout(contains("primary-dirty-file\tshared.txt"))
    .stdout(contains(format!(
        "open-task-dirty-overlap\towner\tshared.txt\t{}",
        worktree.display()
    )))
    .stderr(contains(
        "terminal-check blocked: managed task worktrees remain open",
    ));
}

#[test]
fn blocked_and_failed_closeout_cli_paths_preserve_terminal_exit_contracts() {
    let fixture = TaskRepoFixture::new();
    let primary_head_before = git_output(&fixture.primary, &["rev-parse", "HEAD"]);

    let blocked_worktree = fixture.create_task_worktree("blocked");
    write_file(&blocked_worktree.join("note.txt"), "bundle evidence\n");
    let blocked = run_qcold(
        &blocked_worktree,
        &fixture.fakebin,
        &[
            "task",
            "closeout",
            "--outcome",
            "blocked",
            "--reason",
            "need operator",
        ],
    )
    .assert()
    .code(10);
    let blocked_stdout = String::from_utf8_lossy(&blocked.get_output().stdout);
    let blocked_bundle = PathBuf::from(parse_value("BUNDLE_PATH", &blocked_stdout).unwrap());
    assert!(blocked_bundle.exists());
    assert!(parse_value("RECEIPT_PATH", &blocked_stdout).is_none());
    let blocked_env = bundle_extract_env(&blocked_bundle, terminal_receipt_relative_path());
    assert_eq!(blocked_env.get("OUTCOME").unwrap(), "blocked");
    assert_eq!(blocked_env.get("REASON").unwrap(), "need operator");
    assert_eq!(blocked_env.get("PRIMARY_CHECKOUT_CLEAN").unwrap(), "yes");
    assert_eq!(blocked_env.get("TASK_WORKTREE_REMOVED").unwrap(), "yes");
    assert_eq!(blocked_env.get("LOCAL_TASK_BRANCH_REMOVED").unwrap(), "yes");
    assert!(!blocked_worktree.exists());
    assert_eq!(
        git_output(&fixture.primary, &["rev-parse", "HEAD"]),
        primary_head_before
    );

    let failed_worktree = fixture.create_task_worktree("failed");
    write_file(&failed_worktree.join("failed.txt"), "terminal evidence\n");
    let failed = run_qcold(
        &failed_worktree,
        &fixture.fakebin,
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
    .code(11);
    let failed_stdout = String::from_utf8_lossy(&failed.get_output().stdout);
    let failed_bundle = PathBuf::from(parse_value("BUNDLE_PATH", &failed_stdout).unwrap());
    assert!(failed_bundle.exists());
    assert!(parse_value("RECEIPT_PATH", &failed_stdout).is_none());
    let failed_env = bundle_extract_env(&failed_bundle, terminal_receipt_relative_path());
    assert_eq!(failed_env.get("OUTCOME").unwrap(), "failed");
    assert_eq!(failed_env.get("REASON").unwrap(), "simulated failure");
    assert_eq!(
        failed_env.get("CANONICAL_VALIDATION").unwrap(),
        "not-applicable"
    );
    assert!(!failed_worktree.exists());
    assert_eq!(
        git_output(&fixture.primary, &["rev-parse", "HEAD"]),
        primary_head_before
    );

    run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &["task", "terminal-check"],
    )
    .assert()
    .success();
}

#[test]
fn closeout_from_primary_fails_before_session_start_and_does_not_bundle() {
    let fixture = TaskRepoFixture::new();
    let worktree = fixture.create_task_worktree("wrong-surface");

    let closeout = run_qcold(
        &fixture.primary,
        &fixture.fakebin,
        &[
            "task",
            "closeout",
            "--outcome",
            "success",
            "--message",
            "docs: should not run here",
        ],
    )
    .assert()
    .code(1)
    .stderr(contains(
        "closeout has not started, so no manual bundle is needed for this preflight error",
    ));
    let stdout = String::from_utf8_lossy(&closeout.get_output().stdout);
    assert!(parse_value("BUNDLE_PATH", &stdout).is_none());
    assert!(!fixture.primary.join("bundles").exists());
    assert!(worktree.exists());
}

#[test]
fn closeout_from_task_devcontainer_fails_before_session_start_and_does_not_bundle() {
    let fixture = TaskRepoFixture::new();
    let worktree = fixture.create_task_worktree("wrong-runtime");

    let closeout = run_qcold_in_managed_task_devcontainer(
        &worktree,
        &fixture.primary,
        &fixture.fakebin,
        &[
            "task",
            "closeout",
            "--outcome",
            "success",
            "--message",
            "docs: should not run here either",
        ],
    )
    .assert()
    .code(1)
    .stderr(contains(
        "task closeout must be launched from the host-side managed task worktree shell",
    ))
    .stderr(contains(
        "closeout has not started, so no manual bundle is needed for this preflight error",
    ));
    let stdout = String::from_utf8_lossy(&closeout.get_output().stdout);
    assert!(parse_value("BUNDLE_PATH", &stdout).is_none());
    assert!(!fixture.primary.join("bundles").exists());
    assert!(worktree.exists());
}
