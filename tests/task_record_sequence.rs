#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "integration tests exercise command output contracts"
)]

use assert_cmd::Command as AssertCommand;
use rusqlite::{params, Connection};
use tempfile::tempdir;

fn task_record_create_with_source(
    state_dir: &std::path::Path,
    id: &str,
    repo_root: &str,
    source: &str,
) -> String {
    let output = AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args([
            "task-record",
            "create",
            "--id",
            id,
            "--description",
            "sequence task",
            "--repo-root",
            repo_root,
            "--source",
            source,
        ])
        .env("QCOLD_STATE_DIR", state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).unwrap()
}

fn task_record_create(state_dir: &std::path::Path, id: &str, repo_root: &str) -> String {
    task_record_create_with_source(state_dir, id, repo_root, "manual")
}

fn task_record_delete(state_dir: &std::path::Path, id: &str) {
    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["task-record", "delete", id])
        .env("QCOLD_STATE_DIR", state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();
}

fn pollute_non_task_sequence(state_dir: &std::path::Path, id: &str, repo_root: &str) {
    let db = state_dir.join("qcold.sqlite3");
    let connection = Connection::open(db).unwrap();
    connection
        .execute(
            "update tasks
             set id = 'adhoc/legacy-codex-session',
                 source = 'codex-session',
                 sequence = 500
             where id = ?1",
            [id],
        )
        .unwrap();
    connection
        .execute(
            "update task_sequence_counters
             set next_sequence = 501
             where repo_root = ?1",
            [repo_root],
        )
        .unwrap();
    connection
        .execute(
            "delete from schema_migrations
             where name = 'task_sequence_task_sources_only_v1'",
            [],
        )
        .unwrap();
}

fn task_sequence(state_dir: &std::path::Path, id: &str) -> Option<i64> {
    let db = state_dir.join("qcold.sqlite3");
    let connection = Connection::open(db).unwrap();
    connection
        .query_row(
            "select sequence from tasks where id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn task_record_create_assigns_stable_repo_scoped_sequence() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let repo_a = temp.path().join("repo-a");
    let repo_b = temp.path().join("repo-b");

    let first = task_record_create(&state_dir, "task/first", &repo_a.display().to_string());
    let second = task_record_create(&state_dir, "task/second", &repo_a.display().to_string());
    let repeated = task_record_create(&state_dir, "task/first", &repo_a.display().to_string());
    let other_repo = task_record_create(&state_dir, "task/other", &repo_b.display().to_string());

    assert!(first.contains("\tsequence=1\t"));
    assert!(second.contains("\tsequence=2\t"));
    assert!(repeated.contains("\tsequence=1\t"));
    assert!(other_repo.contains("\tsequence=1\t"));
}

#[test]
fn task_record_repo_move_reallocates_sequence_in_target_repo() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let repo_a = temp.path().join("repo-a");
    let repo_b = temp.path().join("repo-b");

    let occupied = task_record_create(&state_dir, "task/occupied", &repo_b.display().to_string());
    let original = task_record_create(&state_dir, "task/moved", &repo_a.display().to_string());
    let moved = task_record_create(&state_dir, "task/moved", &repo_b.display().to_string());

    assert!(occupied.contains("\tsequence=1\t"));
    assert!(original.contains("\tsequence=1\t"));
    assert!(moved.contains("\tsequence=2\t"));
}

#[test]
fn task_record_sequence_is_not_reused_after_delete() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let repo = temp.path().join("repo");

    let first = task_record_create(&state_dir, "task/first", &repo.display().to_string());
    task_record_delete(&state_dir, "task/first");
    let second = task_record_create(&state_dir, "task/second", &repo.display().to_string());

    assert!(first.contains("\tsequence=1\t"));
    assert!(second.contains("\tsequence=2\t"));
}

#[test]
fn ad_hoc_agent_records_do_not_consume_repo_sequence() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let repo = temp.path().join("repo");

    let agent = task_record_create_with_source(
        &state_dir,
        "adhoc/agent-session",
        &repo.display().to_string(),
        "agent",
    );
    let codex_session = task_record_create_with_source(
        &state_dir,
        "adhoc/codex-session",
        &repo.display().to_string(),
        "codex-session",
    );
    let task = task_record_create(&state_dir, "task/first", &repo.display().to_string());

    assert!(agent.contains("\tsequence=\t"));
    assert!(codex_session.contains("\tsequence=\t"));
    assert!(task.contains("\tsequence=1\t"));
}

#[test]
fn legacy_ad_hoc_sequences_are_scrubbed_before_allocation() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let repo = temp.path().join("repo");
    let repo_root = repo.display().to_string();

    let legacy = task_record_create(&state_dir, "task/legacy", &repo_root);
    assert!(legacy.contains("\tsequence=1\t"));
    pollute_non_task_sequence(&state_dir, "task/legacy", &repo_root);

    let first = task_record_create(&state_dir, "task/first", &repo_root);
    let second = task_record_create(&state_dir, "task/second", &repo_root);

    assert!(first.contains("\tsequence=1\t"));
    assert!(second.contains("\tsequence=2\t"));
    assert_eq!(
        task_sequence(&state_dir, "adhoc/legacy-codex-session"),
        None
    );
}
