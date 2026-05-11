//! Build metadata embedding for the Q-COLD operator binary.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let build_number = git_commit_count().map_or_else(
        || "0".to_string(),
        |count| {
            if git_worktree_dirty() {
                count.saturating_add(1)
            } else {
                count
            }
            .to_string()
        },
    );
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=QCOLD_BUILD_NUMBER={build_number}");
    println!("cargo:rustc-env=QCOLD_BUILD_GIT_HASH={git_hash}");
}

fn git_commit_count() -> Option<u64> {
    Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn git_worktree_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| !output.stdout.is_empty())
}
