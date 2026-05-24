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
            command: vec!["/opt/qcold-test/bin/c1".to_string()],
            cwd: Some(previous.cwd.clone()),
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();

        let context =
            reusable_codex_agent_context("c1", "/opt/qcold-test/bin/c1 resume", None, &primary)
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
                command: vec![format!("/opt/qcold-test/bin/{command}")],
                cwd: Some(previous.cwd.clone()),
                stdout_log_path: None,
                stderr_log_path: None,
            })
            .unwrap();

            let launch = format!("/opt/qcold-test/bin/{command} \"next\"");
            let context = reusable_codex_agent_context(track, &launch, None, &primary)
                .unwrap()
                .unwrap();

            assert_eq!(context.cwd, previous.cwd);
            assert_eq!(context.qcold_repo_root.as_deref(), Some(primary.as_path()));
        }
    }

    #[test]
    fn codex_launch_does_not_reuse_exited_agent_worktree_from_previous_head() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let previous = open_agent_worktree("old", "c1", 100, &primary).unwrap();
        state::insert_agent(&state::AgentRow {
            id: "old".to_string(),
            track: "c1".to_string(),
            pid: u32::MAX,
            started_at: 100,
            command: vec!["/opt/qcold-test/bin/cc1".to_string()],
            cwd: Some(previous.cwd),
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();

        fs::write(primary.join("README.md"), "updated\n").unwrap();
        git_ok(&primary, &["add", "README.md"]);
        git_ok(&primary, &["commit", "-m", "update"]);

        assert!(
            reusable_codex_agent_context("c1", "/opt/qcold-test/bin/cc1 \"next\"", None, &primary)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn codex_launch_does_not_reuse_exited_agent_worktree_from_previous_branch() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let previous = open_agent_worktree("old", "c1", 100, &primary).unwrap();
        git_ok(&previous.cwd, &["checkout", "-b", "queue-ui-end-to-end-fix"]);
        state::insert_agent(&state::AgentRow {
            id: "old".to_string(),
            track: "c1".to_string(),
            pid: u32::MAX,
            started_at: 100,
            command: vec!["/opt/qcold-test/bin/cc1".to_string()],
            cwd: Some(previous.cwd),
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();

        assert!(
            reusable_codex_agent_context("c1", "/opt/qcold-test/bin/cc1 \"next\"", None, &primary)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn codex_launch_from_existing_agent_worktree_stays_in_place() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let current = open_agent_worktree("current", "c1", 100, &primary).unwrap();
        let stale = open_agent_worktree("stale", "c1", 101, &primary).unwrap();
        state::insert_agent(&state::AgentRow {
            id: "stale".to_string(),
            track: "c1".to_string(),
            pid: u32::MAX,
            started_at: 101,
            command: vec!["/opt/qcold-test/bin/cc1".to_string()],
            cwd: Some(stale.cwd),
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();

        let context = prepare_launch_context(
            "new",
            "c1",
            102,
            Some(&current.cwd),
            "/opt/qcold-test/bin/cc1",
        )
        .unwrap();

        assert_eq!(context.cwd, current.cwd);
        assert_eq!(context.qcold_repo_root.as_deref(), Some(primary.as_path()));
        assert_eq!(
            context.qcold_agent_worktree.as_deref(),
            current.qcold_agent_worktree.as_deref()
        );
    }

    #[test]
    fn named_codex_launch_resumes_matching_exited_named_session() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let previous = open_agent_worktree("old", "c1", 100, &primary).unwrap();
        let target = "zellij:qcold-old:1";
        state::insert_agent(&state::AgentRow {
            id: "old".to_string(),
            track: "c1".to_string(),
            pid: u32::MAX,
            started_at: 100,
            command: vec![
                "zellij".to_string(),
                "--session".to_string(),
                "qcold-old".to_string(),
                "pane".to_string(),
                "1".to_string(),
                "/opt/qcold-test/bin/c1".to_string(),
            ],
            cwd: Some(previous.cwd.clone()),
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();
        state::save_terminal_metadata(target, Some("atomic"), None).unwrap();
        let metadata = serde_json::json!({
            "kind": "codex-session-import",
            "command": "/opt/qcold-test/bin/c1",
            "codex_thread_id": "019df1ab-7579-7e41-ad71-701b63175455",
        });
        let record = state::new_task_record(
            "adhoc/old-atomic".to_string(),
            "codex-session".to_string(),
            "Atomic".to_string(),
            "Atomic".to_string(),
            "closed:unknown".to_string(),
            Some(primary.display().to_string()),
            Some(previous.cwd.display().to_string()),
            Some("old".to_string()),
            Some(metadata.to_string()),
        );
        state::upsert_task_record(&record).unwrap();

        let launch = named_codex_resume_launch_for_primary(
            "c1",
            "/opt/qcold-test/bin/c1",
            "Atomic",
            &primary,
        )
        .unwrap()
        .unwrap();

        assert_eq!(launch.cwd, previous.cwd);
        assert_eq!(
            launch.command,
            "'/opt/qcold-test/bin/c1' resume '019df1ab-7579-7e41-ad71-701b63175455'"
        );
    }

    #[test]
    fn named_codex_launch_ignores_explicit_prompt() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);

        assert!(
            named_codex_resume_launch_for_primary(
                "c1",
                "/opt/qcold-test/bin/c1 \"new task\"",
                "atomic",
                &primary,
            )
            .unwrap()
            .is_none()
        );
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
            command: vec!["/opt/qcold-test/bin/cc1".to_string()],
            cwd: Some(running.cwd),
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();

        assert!(
            reusable_codex_agent_context("c1", "/opt/qcold-test/bin/cc1 \"next\"", None, &primary)
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
