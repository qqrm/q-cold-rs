#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "integration tests exercise command output contracts"
)]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use assert_cmd::Command as AssertCommand;
use rusqlite::Connection;
use serde_json::json;
use tempfile::tempdir;

fn git_init(path: &Path) {
    fs::create_dir_all(path).unwrap();
    let status = Command::new("git").arg("init").arg(path).status().unwrap();
    assert!(status.success());
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn shell_quote(value: &Path) -> String {
    format!("'{}'", value.display().to_string().replace('\'', "'\\''"))
}

#[test]
fn sync_remote_imports_task_flow_records_under_local_repo_sequence() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let local_repo = temp.path().join("vitastor");
    git_init(&local_repo);
    let remote = temp.path().join("remote-dev-env");
    let sync_capture = temp.path().join("sync-args.txt");
    let metadata = json!({
        "token_usage": {
            "input_tokens": 10,
            "cached_input_tokens": 4,
            "non_cached_input_tokens": 6,
            "output_tokens": 3,
            "reasoning_output_tokens": 2,
            "total_tokens": 15,
            "displayed_total_tokens": 8,
            "model_calls": 1,
            "model_context_window": 258400,
            "source": "remote-export"
        },
        "token_efficiency": {
            "session_count": 1,
            "matched_by_explicit": 1,
            "matched_by_worktree": 0,
            "matched_by_task": 0,
            "tool_output_original_tokens": 12,
            "large_tool_output_calls": 1,
            "large_tool_output_original_tokens": 9,
            "retention_hours": 48,
            "source": "remote-export"
        }
    });
    let record = json!({
        "id": "task/remote-task",
        "source": "task-flow",
        "sequence": 24,
        "title": "Remote Task",
        "description": "Remote work",
        "status": "open",
        "created_at": 100,
        "updated_at": 200,
        "repo_root": "/remote/vitastor",
        "cwd": "/remote/WT/vitastor/024-remote-task",
        "agent_id": null,
        "metadata_json": metadata.to_string()
    });
    write_executable(
        &remote,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$1\" = env ]; then exit 0; fi\n",
                "printf '%s\\n' \"$@\" > {}\n",
                "printf 'task-record-export\\tcount=1\\n'\n",
                "printf 'task-record-json\\t%s\\n' '{}'\n",
            ),
            shell_quote(&sync_capture),
            record.to_string().replace('\'', "'\\''")
        ),
    );

    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .current_dir(&local_repo)
        .args([
            "task",
            "open-remote",
            "--via",
            remote.to_str().unwrap(),
            "remote-task",
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    let output = AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args([
            "task-record",
            "sync-remote",
            "--via",
            remote.to_str().unwrap(),
            "--local-repo-root",
            local_repo.to_str().unwrap(),
            "--remote-repo-root",
            "/remote/vitastor",
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("remote_records=1\timported=1\tskipped=0"));
    assert_eq!(
        fs::read_to_string(sync_capture)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["cargo", "xtask", "task", "export-records", "--limit", "200"]
    );

    let show = AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["task-record", "show", "task/remote-task"])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let show = String::from_utf8(show).unwrap();
    assert!(show.contains("\tsequence=1\t"));
    assert!(show.contains(&format!("\trepo={}", local_repo.display())));
    assert!(show.contains("\tcwd=/remote/WT/vitastor/024-remote-task\t"));
    assert!(show.contains("token-usage\tinput=10"));
    assert!(show.contains("token-efficiency\tsessions=1"));
}

#[test]
fn sync_remote_preserves_newer_terminal_status() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let local_repo = temp.path().join("vitastor");
    git_init(&local_repo);
    let remote = temp.path().join("remote-dev-env");
    let record = json!({
        "id": "task/remote-terminal-task",
        "source": "task-flow",
        "sequence": 24,
        "title": "Remote Terminal Task",
        "description": "older remote terminal state",
        "status": "closed:success",
        "created_at": 50,
        "updated_at": 100,
        "repo_root": "/remote/vitastor",
        "cwd": "/remote/WT/vitastor/024-remote-terminal-task",
        "agent_id": null,
        "metadata_json": null
    });
    write_executable(
        &remote,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "printf 'task-record-export\\tcount=1\\n'\n",
                "printf 'task-record-json\\t%s\\n' '{}'\n",
            ),
            record.to_string().replace('\'', "'\\''")
        ),
    );

    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args([
            "task-record",
            "create",
            "--id",
            "task/remote-terminal-task",
            "--source",
            "task-flow",
            "--status",
            "closed:blocked",
            "--description",
            "newer terminal state",
            "--repo-root",
            local_repo.to_str().unwrap(),
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args([
            "task-record",
            "sync-remote",
            "--via",
            remote.to_str().unwrap(),
            "--local-repo-root",
            local_repo.to_str().unwrap(),
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    let show = AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["task-record", "show", "task/remote-terminal-task"])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let show = String::from_utf8(show).unwrap();
    assert!(show.contains("\tstatus=closed:blocked\t"), "{show}");
}

#[test]
fn sync_remote_times_out_hung_remote_launcher() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let local_repo = temp.path().join("vitastor");
    git_init(&local_repo);
    let remote = temp.path().join("remote-dev-env");
    write_executable(
        &remote,
        concat!(
            "#!/bin/sh\n",
            "sleep 10\n",
            "printf 'task-record-export\\tcount=0\\n'\n",
        ),
    );

    let assert = AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args([
            "task-record",
            "sync-remote",
            "--via",
            remote.to_str().unwrap(),
            "--local-repo-root",
            local_repo.to_str().unwrap(),
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env("QCOLD_REMOTE_TASK_RECORD_SYNC_TIMEOUT_SECONDS", "1")
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .failure();

    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("timed out after 1s"), "{stderr}");
    assert!(
        stderr.contains("remote repository task-record export"),
        "{stderr}"
    );
}

#[test]
fn sync_remote_retries_transient_task_record_write_lock() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let local_repo = temp.path().join("vitastor");
    git_init(&local_repo);
    let remote = temp.path().join("remote-dev-env");
    let ready = temp.path().join("remote-ready");
    let record = json!({
        "id": "task/remote-lock-task",
        "source": "task-flow",
        "sequence": 24,
        "title": "Remote Lock Task",
        "description": "Remote work",
        "status": "open",
        "created_at": 100,
        "updated_at": 200,
        "repo_root": "/remote/vitastor",
        "cwd": "/remote/WT/vitastor/024-remote-lock-task",
        "agent_id": null,
        "metadata_json": null
    });
    write_executable(
        &remote,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "touch {}\n",
                "sleep 0.2\n",
                "printf 'task-record-export\\tcount=1\\n'\n",
                "printf 'task-record-json\\t%s\\n' '{}'\n",
            ),
            shell_quote(&ready),
            record.to_string().replace('\'', "'\\''")
        ),
    );

    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["task-record", "list", "--limit", "1"])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env("QCOLD_REPO_ROOT", &local_repo)
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    let mut command = Command::new(assert_cmd::cargo::cargo_bin("cargo-qcold"));
    command
        .args([
            "task-record",
            "sync-remote",
            "--via",
            remote.to_str().unwrap(),
            "--local-repo-root",
            local_repo.to_str().unwrap(),
            "--remote-repo-root",
            "/remote/vitastor",
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().unwrap();

    for _ in 0..50 {
        if ready.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists(), "fake remote launcher did not start");

    let db = state_dir.join("qcold.sqlite3");
    let connection = Connection::open(db).unwrap();
    connection.execute("begin immediate", []).unwrap();
    thread::sleep(Duration::from_millis(350));
    connection.execute("commit", []).unwrap();

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "sync failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("remote_records=1\timported=1\tskipped=0"));
}

#[test]
fn legacy_remote_qcold_sync_is_explicit() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let local_repo = temp.path().join("vitastor");
    git_init(&local_repo);
    let remote = temp.path().join("remote-dev-env");
    let capture = temp.path().join("legacy-args.txt");
    let record = json!({
        "id": "task/legacy-remote-task",
        "source": "task-flow",
        "sequence": 8,
        "title": "Legacy Remote Task",
        "description": "Remote work",
        "status": "open",
        "created_at": 100,
        "updated_at": 200,
        "repo_root": "/remote/vitastor",
        "cwd": "/remote/WT/vitastor/008-legacy-remote-task",
        "agent_id": null,
        "metadata_json": null
    });
    write_executable(
        &remote,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$@\" > {}\n",
                "printf 'task-record-json\\t%s\\n' '{}'\n",
            ),
            shell_quote(&capture),
            record.to_string().replace('\'', "'\\''")
        ),
    );

    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args([
            "task-record",
            "sync-remote",
            "--via",
            remote.to_str().unwrap(),
            "--local-repo-root",
            local_repo.to_str().unwrap(),
            "--legacy-remote-qcold",
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(capture)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["qcold", "task-record", "export", "--limit", "200"]
    );
}

#[test]
fn open_remote_reserves_local_sequence_and_passes_it_to_launcher() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let local_repo = temp.path().join("vitastor");
    let capture = temp.path().join("args.txt");
    git_init(&local_repo);
    let remote = temp.path().join("remote-dev-env");
    write_executable(
        &remote,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n",
            shell_quote(&capture)
        ),
    );

    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .current_dir(&local_repo)
        .args([
            "task",
            "open-remote",
            "--via",
            remote.to_str().unwrap(),
            "--remote-task-sequence-env",
            "VITASTOR_TASKFLOW_TASK_SEQUENCE",
            "--remote-task-description-env",
            "VITASTOR_TASKFLOW_DESCRIPTION",
            "remote-sequenced",
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();
    let captured = fs::read_to_string(capture).unwrap();
    assert!(captured.contains("QCOLD_TASK_SEQUENCE=1\n"));
    assert!(captured.contains("VITASTOR_TASKFLOW_TASK_SEQUENCE=1\n"));
    assert!(captured.contains(
        "QCOLD_TASKFLOW_DESCRIPTION=Open managed task-flow work for Remote Sequenced.\n"
    ));
    assert!(captured.contains(
        "VITASTOR_TASKFLOW_DESCRIPTION=Open managed task-flow work for Remote Sequenced.\n"
    ));
    assert!(captured.contains("cargo\n"));
    assert!(captured.contains("xtask\n"));
    assert!(captured.contains("task\n"));
    assert!(captured.contains("open\n"));
    assert!(captured.contains("remote-sequenced\n"));
    assert!(!captured.contains("qcold\n"));

    let show = AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["task-record", "show", "task/remote-sequenced"])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8(show).unwrap().contains("\tsequence=1\t"));

    let export = AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["task-record", "export", "--limit", "10"])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let export = String::from_utf8(export).unwrap();
    assert!(export.contains("remote_launcher"));
    assert!(export.contains("remote_adapter"));
}
