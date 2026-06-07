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
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
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

#[test]
fn mutating_adapter_command_rejects_inherited_repo_root_from_another_checkout() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let inherited_repo = temp.path().join("vitastor");
    let cwd_repo = temp.path().join("qcold");

    seed_git_repo(&inherited_repo);
    seed_git_repo(&cwd_repo);

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args(["verify", "fast"])
        .current_dir(&cwd_repo)
        .env("QCOLD_STATE_DIR", &state_dir)
        .env("QCOLD_REPO_ROOT", &inherited_repo)
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .failure()
        .stderr(contains("repository target mismatch"))
        .stderr(contains(format!(
            "cwd git root is {}",
            cwd_repo.canonicalize().unwrap().display()
        )))
        .stderr(contains(format!(
            "resolved target root is {}",
            inherited_repo.canonicalize().unwrap().display()
        )))
        .stderr(contains("source is QCOLD_REPO_ROOT="));
}

#[test]
fn active_repo_commands_accept_direct_agent_worktree_cwd() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let primary = temp.path().join("qcold");
    let agent_worktree = temp.path().join("WT/qcold/agents/agent-c1-123");

    seed_git_repo(&primary);
    seed_git_repo(&agent_worktree);

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .arg("status")
        .current_dir(&agent_worktree)
        .env("QCOLD_STATE_DIR", &state_dir)
        .env("QCOLD_REPO_ROOT", &primary)
        .env("QCOLD_AGENT_WORKTREE", &agent_worktree)
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .stdout(contains(format!(
            "primary\t{}",
            primary.canonicalize().unwrap().display()
        )));
}

#[test]
fn registered_active_repo_commands_accept_direct_agent_worktree_cwd() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let primary = temp.path().join("qcold");
    let agent_worktree = temp.path().join("WT/qcold/agents/agent-c1-123");

    seed_git_repo(&primary);
    seed_git_repo(&agent_worktree);

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args([
            "repo",
            "add",
            "qcold",
            &primary.display().to_string(),
            "--set-active",
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .arg("status")
        .current_dir(&agent_worktree)
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .stdout(contains(format!(
            "primary\t{}",
            primary.canonicalize().unwrap().display()
        )));
}

#[test]
fn task_flow_command_prefers_registered_cwd_checkout_over_active_repository() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let active_repo = temp.path().join("vitastor");
    let cwd_repo = temp.path().join("qcold");

    seed_git_repo(&active_repo);
    seed_git_repo(&cwd_repo);
    seed_xtask_terminal_check(&active_repo, "vitastor");
    seed_xtask_terminal_check(&cwd_repo, "qcold");
    let active_manifest = active_repo.join("xtask/Cargo.toml").display().to_string();
    let cwd_manifest = cwd_repo.join("xtask/Cargo.toml").display().to_string();

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args([
            "repo",
            "add",
            "vitastor",
            &active_repo.display().to_string(),
            "--xtask-manifest",
            &active_manifest,
            "--set-active",
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args([
            "repo",
            "add",
            "qcold",
            &cwd_repo.display().to_string(),
            "--xtask-manifest",
            &cwd_manifest,
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args(["task", "terminal-check"])
        .current_dir(&cwd_repo)
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success()
        .stdout(contains("terminal-check\tok\tqcold"))
        .stdout(predicates::str::contains("vitastor").not());
}

#[test]
fn adapter_backed_command_rejects_unknown_registered_adapter_id() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let repo = temp.path().join("qcold");

    seed_git_repo(&repo);

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args([
            "repo",
            "add",
            "qcold",
            &repo.display().to_string(),
            "--adapter",
            "custom-adapter",
            "--set-active",
        ])
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .success();

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args(["task", "terminal-check"])
        .current_dir(&repo)
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .failure()
        .stderr(contains(
            "repository qcold uses unsupported adapter custom-adapter",
        ))
        .stderr(contains("supported adapters: xtask-process"));
}

#[test]
fn mutating_adapter_command_rejects_active_repo_from_another_checkout() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let active_repo = temp.path().join("vitastor");
    let cwd_repo = temp.path().join("qcold");

    seed_git_repo(&active_repo);
    seed_git_repo(&cwd_repo);

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

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .args(["verify", "fast"])
        .current_dir(&cwd_repo)
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .failure()
        .stderr(contains("repository target mismatch"))
        .stderr(contains(format!(
            "cwd git root is {}",
            cwd_repo.canonicalize().unwrap().display()
        )))
        .stderr(contains(format!(
            "resolved target root is {}",
            active_repo.canonicalize().unwrap().display()
        )))
        .stderr(contains("source is active repository vitastor"));
}

#[test]
fn status_rejects_active_repo_from_another_checkout() {
    let temp = tempdir().unwrap();
    let state_dir = temp.path().join("state");
    let active_repo = temp.path().join("vitastor");
    let cwd_repo = temp.path().join("qcold");

    seed_git_repo(&active_repo);
    seed_git_repo(&cwd_repo);

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

    AssertCommand::cargo_bin("qcold")
        .unwrap()
        .arg("status")
        .current_dir(&cwd_repo)
        .env("QCOLD_STATE_DIR", &state_dir)
        .env_remove("QCOLD_REPO_ROOT")
        .env_remove("QCOLD_ACTIVE_REPO")
        .assert()
        .failure()
        .stderr(contains("repository target mismatch"))
        .stderr(contains("source is active repository vitastor"));
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

fn seed_xtask_terminal_check(repo: &Path, marker: &str) {
    fs::create_dir_all(repo.join("xtask/src")).unwrap();
    fs::write(
        repo.join("Cargo.toml"),
        "[workspace]\nmembers = [\"xtask\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        repo.join("xtask/Cargo.toml"),
        "[package]\nname = \"xtask\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    )
    .unwrap();
    fs::write(
        repo.join("xtask/src/main.rs"),
        format!(
            "fn main() {{\n\
                 let args: Vec<String> = std::env::args().skip(1).collect();\n\
                 if args == [\"task\", \"terminal-check\"] {{\n\
                     println!(\"terminal-check\\tok\\t{marker}\");\n\
                     return;\n\
                 }}\n\
                 eprintln!(\"unexpected args: {{args:?}}\");\n\
                 std::process::exit(1);\n\
             }}\n"
        ),
    )
    .unwrap();
    git(repo, &["add", "Cargo.toml", "xtask"]);
    git(repo, &["commit", "-m", "add xtask fixture"]);
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
