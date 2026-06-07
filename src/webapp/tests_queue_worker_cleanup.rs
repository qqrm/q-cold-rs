#[cfg(test)]
mod queue_worker_cleanup_tests {
    use super::*;

    #[test]
    fn queue_agent_cleanup_deletes_stale_record_after_terminal_cleanup_failure() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let agent = state::AgentRow {
            id: "qa-stale-cleanup".to_string(),
            track: "queue-test".to_string(),
            pid: u32::MAX,
            started_at: unix_now(),
            command: vec![
                "tmux".to_string(),
                "new-session".to_string(),
                "-s".to_string(),
                "qcold-qa-stale-cleanup".to_string(),
                "c1".to_string(),
            ],
            cwd: None,
            stdout_log_path: None,
            stderr_log_path: None,
        };
        state::insert_agent(&agent).unwrap();

        let message = cleanup_queue_agent(&agent.id);
        let agents = state::load_agents(&temp.path().join("legacy-agents.tsv")).unwrap();

        assert!(message.contains("agent record deleted"));
        assert!(!agents.iter().any(|row| row.id == agent.id));
    }

    #[test]
    fn queue_launch_cleanup_removes_stale_record_and_clean_agent_worktree() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        seed_git_repo(&repo);
        let item = queue_item_fixture("task-stale-agent-cleanup", &repo);
        let agent_id = queue_agent_id(&item);
        let worktree =
            agents::agent_worktree_path_for_launch_id(&agent_id, &queue_track(&item.run_id), 0, &repo)
                .unwrap();
        create_git_worktree(&repo, &worktree);
        state::insert_agent(&agent_fixture(&agent_id, Some(worktree.clone()))).unwrap();

        cleanup_stale_queue_agent_launch_artifacts(&item, &repo).unwrap();

        assert!(!worktree.exists());
        let agents = state::load_agents(&temp.path().join("legacy-agents.tsv")).unwrap();
        assert!(!agents.iter().any(|row| row.id == agent_id));
    }

    #[test]
    fn queue_launch_cleanup_refuses_dirty_agent_worktree() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        seed_git_repo(&repo);
        let item = queue_item_fixture("task-dirty-agent-cleanup", &repo);
        let agent_id = queue_agent_id(&item);
        let worktree =
            agents::agent_worktree_path_for_launch_id(&agent_id, &queue_track(&item.run_id), 0, &repo)
                .unwrap();
        create_git_worktree(&repo, &worktree);
        fs::write(worktree.join("dirty.txt"), "dirty\n").unwrap();
        state::insert_agent(&agent_fixture(&agent_id, Some(worktree.clone()))).unwrap();

        let err = cleanup_stale_queue_agent_launch_artifacts(&item, &repo).unwrap_err();

        assert!(
            format!("{err:#}").contains("has local changes"),
            "{err:#}"
        );
        assert!(worktree.exists());
        let agents = state::load_agents(&temp.path().join("legacy-agents.tsv")).unwrap();
        assert!(agents.iter().any(|row| row.id == agent_id));
    }

    #[test]
    #[cfg(unix)]
    fn remote_native_port_forward_failure_runs_best_effort_cleanup() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let log = install_fake_cargo_logger(temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_item_fixture("task-remote-native-port-cleanup", &repo);
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some("remote-dev-env".to_string());
        item.remote_agent_remote_proxy = Some("127.0.0.1:18330".to_string());

        let cleanup = cleanup_remote_native_port_forward_failure(
            &item,
            concat!(
                "repository remote-agent open contract failed with exit status: 255: ",
                "Error: remote port forwarding failed for listen port 18330",
            ),
        );

        assert_eq!(cleanup.as_deref(), Some("remote-agent session stopped"));
        let log = fs::read_to_string(log).unwrap();
        assert!(log.contains(
            "xtask remote-agent down --session qcold-qa-task-remote-native-port-cleanup"
        ));
        assert!(log.contains("--remote-proxy 127.0.0.1:18330"));
        assert!(log.contains("QCOLD_REMOTE_DEV_ENV_WRAPPER=remote-dev-env"));
    }

    fn queue_item_fixture(slug: &str, repo: &Path) -> state::QueueItemRow {
        state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "do focused work".to_string(),
            slug: slug.to_string(),
            repo_root: Some(repo.display().to_string()),
            repo_name: Some("repo".to_string()),
            execution_host: "local".into(),
            agent_command: "c1".to_string(),
            task_class: state::QueueTaskClass::Mid,
            remote_launcher: None,
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            agent_id: None,
            status: "pending".into(),
            message: String::new(),
            attempts: 0,
            recovery_attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        }
    }

    fn agent_fixture(id: &str, cwd: Option<PathBuf>) -> state::AgentRow {
        state::AgentRow {
            id: id.to_string(),
            track: "queue-run".to_string(),
            pid: u32::MAX,
            started_at: unix_now(),
            command: vec![
                "tmux".to_string(),
                "new-session".to_string(),
                "-s".to_string(),
                format!("qcold-{id}"),
                "c1".to_string(),
            ],
            cwd,
            stdout_log_path: None,
            stderr_log_path: None,
        }
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

    fn create_git_worktree(repo: &Path, worktree: &Path) {
        fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        let worktree_arg = worktree.display().to_string();
        git_ok(repo, &["worktree", "add", "--detach", &worktree_arg, "HEAD"]);
    }

    #[cfg(unix)]
    fn install_fake_cargo_logger(temp: &Path) -> PathBuf {
        let bin = temp.join("bin");
        fs::create_dir(&bin).unwrap();
        let log = temp.join("cargo.log");
        let cargo = bin.join("cargo");
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$PWD|$*|QCOLD_REMOTE_DEV_ENV_WRAPPER=$QCOLD_REMOTE_DEV_ENV_WRAPPER\" \
             >> {}\n",
            shell_quote(&log)
        );
        fs::write(&cargo, script).unwrap();
        make_executable(&cargo);

        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&path));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        log
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(unix)]
    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    fn git_ok(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {} failed", args.join(" "));
    }
}
