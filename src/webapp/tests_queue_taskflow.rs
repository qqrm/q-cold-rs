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
    fn queue_task_status_ignores_closed_record_from_other_repo() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo_a = temp.path().join("repo-a");
        let repo_b = temp.path().join("repo-b");
        fs::create_dir_all(&repo_a).unwrap();
        fs::create_dir_all(&repo_b).unwrap();
        let item = queue_taskflow_item("shared-slug", &repo_a, None);
        state::upsert_task_record(&task_record_fixture(
            "shared-slug",
            "closed:success",
            &repo_b,
        ))
        .unwrap();

        assert_eq!(queue_task_status(&item).unwrap(), None);
    }

    #[test]
    fn queue_launch_workspace_rejects_live_slug_conflict() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo_a = temp.path().join("repo-a");
        let repo_b = temp.path().join("repo-b");
        fs::create_dir_all(&repo_a).unwrap();
        fs::create_dir_all(&repo_b).unwrap();
        let run = queue_run_fixture("run-a", &repo_a);
        let mut existing = queue_taskflow_item("shared-slug", &repo_a, None);
        existing.id = "item-a".to_string();
        existing.run_id = run.id.clone();
        existing.status = "running".to_string();
        state::replace_web_queue(&run, &[existing]).unwrap();
        let candidate = queue_taskflow_item("shared-slug", &repo_b, None);

        let err = match queue_launch_workspace(&candidate) {
            Ok(_) => panic!("slug conflict should be rejected"),
            Err(err) => err,
        };

        assert!(format!("{err:#}").contains("already active"));
    }

    #[test]
    fn queue_launch_workspace_ignores_discovered_worktree_for_other_repo() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo_a = temp.path().join("repo-a");
        let repo_b = temp.path().join("repo-b");
        fs::create_dir_all(&repo_a).unwrap();
        fs::create_dir_all(&repo_b).unwrap();
        let stale_worktree = temp.path().join("WT/repo-a/001-shared-slug");
        write_task_env(&stale_worktree, "shared-slug", &repo_b);
        let item = queue_taskflow_item("shared-slug", &repo_a, None);

        let workspace = queue_launch_workspace(&item).unwrap();

        assert_eq!(workspace.worktree, repo_a.canonicalize().unwrap());
        assert!(!workspace.existing_task);
    }

    #[test]
    fn queue_launch_workspace_ignores_record_cwd_for_other_repo() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo_a = temp.path().join("repo-a");
        let repo_b = temp.path().join("repo-b");
        fs::create_dir_all(&repo_a).unwrap();
        fs::create_dir_all(&repo_b).unwrap();
        let stale_worktree = temp.path().join("stale-worktree");
        write_task_env(&stale_worktree, "shared-slug", &repo_b);
        state::upsert_task_record(&state::TaskRecordRow {
            cwd: Some(stale_worktree.display().to_string()),
            ..task_record_fixture("shared-slug", "open", &repo_a)
        })
        .unwrap();
        let item = queue_taskflow_item("shared-slug", &repo_a, None);

        let workspace = queue_launch_workspace(&item).unwrap();

        assert_eq!(workspace.worktree, repo_a.canonicalize().unwrap());
        assert!(!workspace.existing_task);
    }

    #[test]
    fn queue_cleanup_keeps_task_record_from_other_repo() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo_a = temp.path().join("repo-a");
        let repo_b = temp.path().join("repo-b");
        fs::create_dir_all(&repo_a).unwrap();
        fs::create_dir_all(&repo_b).unwrap();
        let item = queue_taskflow_item("shared-slug", &repo_a, None);
        state::upsert_task_record(&task_record_fixture("shared-slug", "open", &repo_b)).unwrap();

        cleanup_queue_item_artifacts(&item, None, None).unwrap();

        assert!(state::get_task_record("task/shared-slug").unwrap().is_some());
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

    fn queue_run_fixture(id: &str, repo: &Path) -> state::QueueRunRow {
        state::QueueRunRow {
            id: id.to_string(),
            status: "running".to_string(),
            execution_mode: "sequence".to_string(),
            selected_agent_command: "c1".to_string(),
            remote_launcher: None,
            selected_repo_root: Some(repo.display().to_string()),
            selected_repo_name: Some("repo".to_string()),
            track: queue_track(id),
            current_index: -1,
            stop_requested: false,
            message: "queued".to_string(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn task_record_fixture(slug: &str, status: &str, repo: &Path) -> state::TaskRecordRow {
        state::new_task_record(
            format!("task/{slug}"),
            "task-flow".to_string(),
            slug.to_string(),
            "existing task".to_string(),
            status.to_string(),
            Some(repo.display().to_string()),
            Some(repo.join("WT").join(slug).display().to_string()),
            None,
            None,
        )
    }

    fn write_task_env(worktree: &Path, slug: &str, repo: &Path) {
        fs::create_dir_all(worktree.join(".task")).unwrap();
        fs::write(
            worktree.join(".task/task.env"),
            format!("TASK_NAME={slug}\nPRIMARY_REPO_PATH={}\n", repo.display()),
        )
        .unwrap();
    }

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
