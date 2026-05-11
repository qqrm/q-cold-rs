#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "integration tests exercise command output contracts"
)]

use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::Command as AssertCommand;
use tempfile::tempdir;

#[test]
fn codex_agent_launch_prefers_daemon_cwd_checkout_over_active_repository() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let active_repo = temp.path().join("vitastor");
    let daemon_repo = temp.path().join("qcold");
    let capture = temp.path().join("capture.env");
    let bin_dir = temp.path().join("bin");
    let fake_codex = bin_dir.join("codex");

    seed_git_repo(&active_repo);
    seed_git_repo(&daemon_repo);
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(
        &fake_codex,
        format!(
            "#!/bin/sh\n\
             printf 'pwd=%s\\nrepo=%s\\nagent=%s\\n' \
             \"$PWD\" \"$QCOLD_REPO_ROOT\" \"$QCOLD_AGENT_WORKTREE\" > {}\n",
            shell_quote(&capture.display().to_string())
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_codex, fs::Permissions::from_mode(0o755)).unwrap();
    }

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args([
            "repo",
            "add",
            "vitastor",
            &active_repo.display().to_string(),
            "--set-active",
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args([
            "agent", "start", "--track", "work3", "--", "codex", "exec", "status",
        ])
        .current_dir(&daemon_repo)
        .env("PATH", path)
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    let captured = wait_for_capture(&capture);
    assert!(captured.contains(&format!("repo={}", daemon_repo.display())));
    assert!(captured.contains("/WT/qcold/agents/"));
    assert!(!captured.contains(&format!("repo={}", active_repo.display())));
}

fn seed_git_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init"]);
    git(path, &["config", "user.email", "qcold@example.invalid"]);
    git(path, &["config", "user.name", "Q-COLD Test"]);
    fs::write(path.join("README.md"), "seed\n").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "-m", "seed"]);
}

fn git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed with {status}", args);
}

fn wait_for_capture(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(contents) = fs::read_to_string(path) {
            return contents;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
