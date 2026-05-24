#![allow(
    missing_docs,
    clippy::unwrap_used,
    reason = "integration tests assert command-output behavior"
)]

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;

#[test]
fn guard_blocks_oversized_stdout_and_stderr_without_raw_payload() {
    let mut command = AssertCommand::cargo_bin("cargo-qcold").unwrap();
    command.args([
        "guard",
        "--max-bytes",
        "8",
        "--max-lines",
        "100",
        "sh",
        "-c",
        "printf STDOUT_RAW_PAYLOAD; printf STDERR_RAW_PAYLOAD >&2",
    ]);

    command
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("qcold-guard\tstatus=blocked"))
        .stderr(predicate::str::contains("rerun with a narrower command"))
        .stderr(predicate::str::contains("STDOUT_RAW_PAYLOAD").not())
        .stderr(predicate::str::contains("STDERR_RAW_PAYLOAD").not());
}

#[test]
fn guard_allows_small_output_unchanged() {
    let mut command = AssertCommand::cargo_bin("cargo-qcold").unwrap();
    command.args([
        "guard",
        "--max-bytes",
        "64",
        "--max-lines",
        "4",
        "sh",
        "-c",
        "printf small-out; printf small-err >&2",
    ]);

    command
        .assert()
        .success()
        .stdout("small-out")
        .stderr("small-err");
}
