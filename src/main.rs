#![allow(
    missing_docs,
    reason = "qcold is an incubating operator facade with an adapter-backed command surface"
)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::process::ExitCode;

fn main() -> ExitCode {
    qcold::run_cli()
}
