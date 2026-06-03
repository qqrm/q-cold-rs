#![allow(
    missing_docs,
    clippy::unwrap_used,
    reason = "integration tests exercise command output contracts"
)]

use assert_cmd::Command as AssertCommand;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::{contains, is_match};

const QCOLD_VERSION_PATTERN: &str = r"qcold \d+\.\d+\.\d+\.\d+ [0-9a-f]{12}(-dirty)?\n";

#[test]
fn qcold_reports_package_version() {
    let package_version = env!("CARGO_PKG_VERSION");
    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(format!("qcold {package_version}.")))
        .stdout(is_match(QCOLD_VERSION_PATTERN).unwrap());
}

#[test]
fn cargo_subcommand_reports_package_version() {
    let package_version = env!("CARGO_PKG_VERSION");
    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["qcold", "--version"])
        .assert()
        .success()
        .stdout(contains(format!("qcold {package_version}.")))
        .stdout(is_match(QCOLD_VERSION_PATTERN).unwrap());
}

#[test]
fn default_help_hides_advanced_compatibility_surface() {
    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Core examples:"))
        .stdout(contains("queue"))
        .stdout(contains("task"))
        .stdout(contains("task-record").not())
        .stdout(contains("q-help").not())
        .stdout(contains("bundle").not())
        .stdout(contains("guard").not())
        .stdout(contains("verify").not())
        .stdout(contains("  compat").not());
}

#[test]
fn task_help_hides_cleanup_subcommands() {
    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args(["task", "--help"])
        .assert()
        .success()
        .stdout(contains("closeout"))
        .stdout(contains("clear-all").not())
        .stdout(contains("orphan-clear-stale").not())
        .stdout(contains("iteration-notify").not());
}

#[test]
fn agent_help_hides_cleanup_subcommands() {
    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args(["agent", "--help"])
        .assert()
        .success()
        .stdout(contains("attach"))
        .stdout(contains("named-sessions").not())
        .stdout(contains("prune-stale").not());
}

#[test]
fn telegram_help_shows_dashboard_entrypoint_only() {
    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args(["telegram", "--help"])
        .assert()
        .success()
        .stdout(contains("serve"))
        .stdout(contains("poll").not());
}

#[test]
fn q_help_prints_queue_package_guidance() {
    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .arg("q-help")
        .assert()
        .success()
        .stdout(contains("qcold queue run --from queue.json"))
        .stdout(contains("layers/*.md"));
}

#[test]
fn queue_help_exposes_console_queue_commands() {
    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args(["queue", "--help"])
        .assert()
        .success()
        .stdout(contains("Submit a new queue run"))
        .stdout(contains("Create an empty queue tab"))
        .stdout(contains("make it active").not())
        .stdout(contains("Append prompt items"));
}
