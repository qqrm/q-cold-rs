#![allow(
    missing_docs,
    clippy::unwrap_used,
    reason = "integration tests exercise command output contracts"
)]

use assert_cmd::Command as AssertCommand;
use predicates::str::contains;

#[test]
fn qcold_reports_package_version() {
    let package_version = env!("CARGO_PKG_VERSION");
    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(format!("qcold {package_version} ")));
}

#[test]
fn cargo_subcommand_reports_package_version() {
    let package_version = env!("CARGO_PKG_VERSION");
    AssertCommand::cargo_bin("cargo-qcold")
        .unwrap()
        .args(["qcold", "--version"])
        .assert()
        .success()
        .stdout(contains(format!("qcold {package_version} ")));
}
