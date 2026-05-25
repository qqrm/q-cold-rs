#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "integration tests exercise command output contracts"
)]

use std::fs;
use std::process::{Command, Stdio};

use assert_cmd::Command as AssertCommand;
use tempfile::tempdir;

#[test]
fn task_record_list_discovers_managed_task_env_with_sequence() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let repo = temp.path().join("repo");
    let worktree = temp.path().join("WT/repo/042-task-backed-by-env");
    fs::create_dir_all(worktree.join(".task")).unwrap();
    fs::create_dir_all(&repo).unwrap();
    fs::write(
        worktree.join(".task/task.env"),
        format!(
            "TASK_ID='task/task-backed-by-env'\n\
             TASK_NAME='task-backed-by-env'\n\
             TASK_SEQUENCE='42'\n\
             TASK_EXECUTION_ANCHOR='42'\n\
             TASK_DESCRIPTION='Managed task from env.'\n\
             TASK_WORKTREE='{}'\n\
             PRIMARY_REPO_PATH='{}'\n\
             STATUS='open'\n",
            worktree.display(),
            repo.display(),
        ),
    )
    .unwrap();

    let output = AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["task-record", "list", "--limit", "10"])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env("QCOLD_REPO_ROOT", &repo)
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();

    assert!(output.contains(
        "task-record\ttask/task-backed-by-env\tsequence=42\tstatus=open\tsource=task-flow"
    ));
    assert!(output.contains(&format!("repo={}", repo.display())));
    assert!(output.contains(&format!("cwd={}", worktree.display())));
}

#[test]
fn task_record_list_closes_managed_task_env_terminal_status() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let repo = temp.path().join("repo");
    let worktree = temp.path().join("WT/repo/043-task-backed-by-terminal-env");
    fs::create_dir_all(worktree.join(".task")).unwrap();
    fs::create_dir_all(&repo).unwrap();
    fs::write(
        worktree.join(".task/task.env"),
        format!(
            "TASK_ID='task/task-backed-by-terminal-env'\n\
             TASK_NAME='task-backed-by-terminal-env'\n\
             TASK_SEQUENCE='43'\n\
             TASK_DESCRIPTION='Managed task from terminal env.'\n\
             TASK_WORKTREE='{}'\n\
             PRIMARY_REPO_PATH='{}'\n\
             STATUS='success'\n",
            worktree.display(),
            repo.display(),
        ),
    )
    .unwrap();

    let output = AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["task-record", "list", "--limit", "10"])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env("QCOLD_REPO_ROOT", &repo)
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();

    assert!(output.contains(
        "task-record\ttask/task-backed-by-terminal-env\tsequence=43\tstatus=closed:success"
    ));
}

#[test]
fn task_record_list_closes_from_terminal_bundle_after_worktree_cleanup() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let repo = temp.path().join("repo");
    let old_worktree = temp.path().join("WT/repo/044-task-closed-by-bundle");
    fs::create_dir_all(repo.join("bundles")).unwrap();
    write_terminal_bundle(&repo, "task-closed-by-bundle", 1_860);

    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args([
            "task-record",
            "create",
            "--id",
            "task/task-closed-by-bundle",
            "--source",
            "task-flow",
            "--repo-root",
            repo.to_str().unwrap(),
            "--cwd",
            old_worktree.to_str().unwrap(),
            "--description",
            "Task closed by terminal bundle.",
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env("QCOLD_REPO_ROOT", &repo)
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    let output = AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["task-record", "show", "task/task-closed-by-bundle"])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env("QCOLD_REPO_ROOT", &repo)
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();

    assert!(output
        .contains("task-record\ttask/task-closed-by-bundle\tsequence=1\tstatus=closed:success"));
    assert!(output.contains("repo="));

    let db = state_dir.join("qcold.sqlite3");
    let connection = rusqlite::Connection::open(db).unwrap();
    let metadata: String = connection
        .query_row(
            "select metadata_json from tasks where id = 'task/task-closed-by-bundle'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(
        metadata
            .get("task_duration_seconds")
            .and_then(serde_json::Value::as_u64),
        Some(1_860)
    );
    assert!(metadata
        .get("task_terminal_bundle")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|path| path.ends_with("task-closed-by-bundle_success.zip")));
}

fn write_terminal_bundle(repo: &std::path::Path, task_name: &str, duration_seconds: u64) {
    let stage = tempdir().unwrap();
    fs::create_dir_all(stage.path().join("metadata")).unwrap();
    fs::write(
        stage.path().join("metadata/terminal-receipt.env"),
        format!(
            "OUTCOME='success'\n\
             TASK_ID='task/{task_name}'\n\
             TASK_NAME='{task_name}'\n"
        ),
    )
    .unwrap();
    fs::write(
        stage.path().join("metadata/bundle.env"),
        format!(
            "TASK_ID='task/{task_name}'\n\
             TASK_NAME='{task_name}'\n\
             TASK_DURATION_SECONDS='{duration_seconds}'\n"
        ),
    )
    .unwrap();
    let bundle = repo
        .join("bundles")
        .join(format!("{task_name}_success.zip"));
    let status = Command::new("7z")
        .current_dir(stage.path())
        .args(["a", "-tzip", bundle.to_str().unwrap(), "metadata"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "failed to create terminal bundle fixture");
}
