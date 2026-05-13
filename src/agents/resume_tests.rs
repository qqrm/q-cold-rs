#[cfg(test)]
mod resume_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use tempfile::tempdir;

    fn git_ok(cwd: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .current_dir(cwd)
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git command failed in {}: {:?}",
            cwd.display(),
            args
        );
    }

    fn seed_git_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        git_ok(path, &["init"]);
        git_ok(path, &["config", "user.name", "tester"]);
        git_ok(path, &["config", "user.email", "tester@example.com"]);
        fs::write(path.join("README.md"), "seed\n").unwrap();
        git_ok(path, &["add", "README.md"]);
        git_ok(path, &["commit", "-m", "seed"]);
    }

    #[test]
    fn codex_resume_reuses_latest_agent_worktree_for_same_track() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let previous = open_agent_worktree("old", "c1", 100, &primary).unwrap();
        state::insert_agent(&state::AgentRow {
            id: "old".to_string(),
            track: "c1".to_string(),
            pid: 0,
            started_at: 100,
            command: vec!["/home/qqrm/.local/bin/c1".to_string()],
            cwd: Some(previous.cwd.clone()),
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();

        let context =
            reusable_codex_agent_context("c1", "/home/qqrm/.local/bin/c1 resume", None, &primary)
                .unwrap()
                .unwrap();

        assert_eq!(context.cwd, previous.cwd);
        assert_eq!(context.qcold_repo_root.as_deref(), Some(primary.as_path()));
        assert_eq!(
            context.qcold_agent_worktree.as_deref(),
            Some(previous.qcold_agent_worktree.as_deref().unwrap())
        );
    }

    #[test]
    fn codex_launch_reuses_latest_exited_agent_worktree_for_same_track() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));

        for (track, command, started_at) in [("c1", "cc1", 100), ("c2", "cc2", 101)] {
            let primary = temp.path().join(format!("repo-{track}"));
            seed_git_repo(&primary);
            let id = format!("old-{track}");
            let previous = open_agent_worktree(&id, track, started_at, &primary).unwrap();
            state::insert_agent(&state::AgentRow {
                id,
                track: track.to_string(),
                pid: u32::MAX,
                started_at,
                command: vec![format!("/home/qqrm/.local/bin/{command}")],
                cwd: Some(previous.cwd.clone()),
                stdout_log_path: None,
                stderr_log_path: None,
            })
            .unwrap();

            let launch = format!("/home/qqrm/.local/bin/{command} \"next\"");
            let context = reusable_codex_agent_context(track, &launch, None, &primary)
                .unwrap()
                .unwrap();

            assert_eq!(context.cwd, previous.cwd);
            assert_eq!(context.qcold_repo_root.as_deref(), Some(primary.as_path()));
        }
    }

    #[test]
    fn codex_launch_does_not_reuse_running_agent_worktree() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let running = open_agent_worktree("running", "c1", 100, &primary).unwrap();
        state::insert_agent(&state::AgentRow {
            id: "running".to_string(),
            track: "c1".to_string(),
            pid: std::process::id(),
            started_at: 100,
            command: vec!["/home/qqrm/.local/bin/cc1".to_string()],
            cwd: Some(running.cwd),
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();

        assert!(
            reusable_codex_agent_context("c1", "/home/qqrm/.local/bin/cc1 \"next\"", None, &primary)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn codex_exec_launch_does_not_reuse_exited_agent_worktree() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let previous = open_agent_worktree("old", "audit", 100, &primary).unwrap();
        state::insert_agent(&state::AgentRow {
            id: "old".to_string(),
            track: "audit".to_string(),
            pid: u32::MAX,
            started_at: 100,
            command: vec!["codex".to_string(), "exec".to_string(), "inspect".to_string()],
            cwd: Some(previous.cwd),
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();

        assert!(
            reusable_codex_agent_context("audit", "codex exec inspect", None, &primary)
                .unwrap()
                .is_none()
        );
    }
}
