#[cfg(test)]
mod queue_taskflow_tests {
    use crate::test_support;

    use super::*;

    #[test]
    fn queue_task_open_prefers_runnable_current_executable() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current-qcold");
        fs::write(&current, "").unwrap();
        make_executable(&current);

        assert_eq!(
            queue_qcold_executable_from(&current, None).unwrap(),
            current
        );
    }

    #[test]
    fn queue_task_open_falls_back_to_path_when_current_executable_was_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let installed = bin.join(format!("qcold{}", env::consts::EXE_SUFFIX));
        fs::write(&installed, "").unwrap();
        make_executable(&installed);

        let missing_current = temp.path().join("qcold (deleted)");
        let resolved = queue_qcold_executable_from(
            &missing_current,
            Some(std::ffi::OsStr::new(bin.to_str().unwrap())),
        )
        .unwrap();

        assert_eq!(resolved, installed);
    }

    #[test]
    fn queue_task_env_value_accepts_shell_quotes() {
        assert_eq!(shell_env_value("'task-run-01'"), "task-run-01");
        assert_eq!(shell_env_value("'task-'\\''run'"), "task-'run");
        assert_eq!(shell_env_value("task-run-01"), "task-run-01");
    }

    #[test]
    fn queue_launch_workspace_without_existing_task_uses_repo_root() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let item = queue_taskflow_item("task-run-01", &repo, None);

        let workspace = queue_launch_workspace(&item).unwrap();

        assert_eq!(workspace.worktree, repo.canonicalize().unwrap());
        assert_eq!(workspace.remote_launcher, None);
        assert_eq!(workspace.remote_worktree, None);
        assert!(!workspace.existing_task);
    }

    #[test]
    fn queue_launch_workspace_preserves_remote_launcher_without_opening_task() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let item = queue_taskflow_item("task-remote-01", &repo, Some("remote-dev-env"));

        let workspace = queue_launch_workspace(&item).unwrap();

        assert_eq!(workspace.worktree, repo.canonicalize().unwrap());
        assert_eq!(workspace.remote_launcher.as_deref(), Some("remote-dev-env"));
        assert_eq!(workspace.remote_worktree, None);
        assert!(!workspace.existing_task);
        assert!(state::get_task_record("task/task-remote-01").unwrap().is_none());
    }

    #[test]
    fn queue_remote_launcher_is_explicit_not_agents_autoselected() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        fs::write(
            repo.join("AGENTS.md"),
            "The default substantive execution environment is the approved remote dev environment.",
        )
        .unwrap();
        std::env::remove_var("QCOLD_QUEUE_REMOTE_LAUNCHER");

        assert_eq!(
            resolve_queue_remote_launcher(None, Some(repo.to_str().unwrap())),
            None
        );

        std::env::set_var("QCOLD_QUEUE_REMOTE_LAUNCHER", "remote-dev-env");
        assert_eq!(
            resolve_queue_remote_launcher(None, Some(repo.to_str().unwrap())),
            Some("remote-dev-env".to_string())
        );
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}

    fn queue_taskflow_item(
        slug: &str,
        repo: &Path,
        remote_launcher: Option<&str>,
    ) -> state::QueueItemRow {
        state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "do focused work".to_string(),
            slug: slug.to_string(),
            repo_root: Some(repo.display().to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            remote_launcher: remote_launcher.map(str::to_string),
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        }
    }
}
