//! Build metadata embedding for the Q-COLD operator binary.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let build_number =
        git_commit_count().map_or_else(|| "0".to_string(), |count| count.to_string());
    let mut git_hash = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    if git_tracked_worktree_dirty() && git_hash != "unknown" {
        git_hash.push_str("-dirty");
    }

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

fn git_tracked_worktree_dirty() -> bool {
    git_has_diff(&["diff", "--quiet"]) || git_has_diff(&["diff", "--cached", "--quiet"])
}

fn git_has_diff(args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .status()
        .ok()
        .is_some_and(|status| status.code() == Some(1))
}
