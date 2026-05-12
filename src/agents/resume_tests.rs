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
            reusable_codex_resume_context("c1", "/home/qqrm/.local/bin/c1 resume", None, &primary)
                .unwrap()
                .unwrap();

        assert_eq!(context.cwd, previous.cwd);
        assert_eq!(context.qcold_repo_root.as_deref(), Some(primary.as_path()));
        assert_eq!(
            context.qcold_agent_worktree.as_deref(),
            Some(previous.qcold_agent_worktree.as_deref().unwrap())
        );
    }
}
