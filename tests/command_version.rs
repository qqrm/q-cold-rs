#![allow(
    missing_docs,
    clippy::unwrap_used,
    reason = "integration tests exercise command output contracts"
)]

use assert_cmd::Command as AssertCommand;
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
        .stdout(contains("Append prompt items"));
}
