#![allow(
    missing_docs,
    clippy::unwrap_used,
    reason = "integration tests exercise command output contracts"
)]

use assert_cmd::Command as AssertCommand;
use predicates::str::{contains, is_match};

#[test]
fn qcold_reports_package_version() {
    let package_version = env!("CARGO_PKG_VERSION");
    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(format!("qcold {package_version}.")))
        .stdout(is_match(r"qcold \d+\.\d+\.\d+\.\d+ [0-9a-f]{12}\n").unwrap());
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
        .stdout(is_match(r"qcold \d+\.\d+\.\d+\.\d+ [0-9a-f]{12}\n").unwrap());
}
