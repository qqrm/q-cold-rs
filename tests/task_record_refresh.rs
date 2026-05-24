#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "integration tests exercise command output contracts"
)]

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use assert_cmd::Command as AssertCommand;
use predicates::str::contains;
use rusqlite::{params, Connection};
use tempfile::tempdir;

#[test]
fn task_record_list_warns_and_continues_when_codex_refresh_fails() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();

    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args([
            "task-record",
            "create",
            "--id",
            "task/visible",
            "--description",
            "visible task",
            "--repo-root",
            repo.to_str().unwrap(),
        ])
        .current_dir(&repo)
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    let sessions_parent = home.join(".codex-accounts/2");
    fs::create_dir_all(&sessions_parent).unwrap();
    fs::write(sessions_parent.join("sessions"), "not a directory").unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let connection = Connection::open(state_dir.join("qcold.sqlite3")).unwrap();
    connection
        .execute(
            "insert into agents
                 (id, track, pid, started_at_unix, command_json, cwd, created_at_unix)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?4)",
            params![
                "agent-refresh-fails",
                "manual",
                i64::from(std::process::id()),
                now,
                r#"["/opt/qcold-test/bin/c2","inspect"]"#,
                repo.to_str().unwrap(),
            ],
        )
        .unwrap();

    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["task-record", "list", "--limit", "10"])
        .current_dir(&repo)
        .env("QCOLD_STATE_DIR", &state_dir)
        .env("CODEX_HOME", &sessions_parent)
        .env("HOME", &home)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .stdout(contains(
            "task-record\ttask/visible\tsequence=1\tstatus=open\tsource=manual",
        ))
        .stderr(contains(
            "warning: failed to refresh Codex task token telemetry",
        ));
}
