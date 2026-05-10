#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "integration tests exercise command output contracts"
)]

use std::fs;

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
