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
    fn closed_failed_queue_task_schedules_one_auto_recovery() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("auto-recovery", &repo);
        let mut item = queue_taskflow_item("task-auto-recovery", &repo, None);
        item.run_id = run.id.clone();
        item.status = "running".to_string();
        item.agent_id = Some("agent-failed".to_string());
        state::replace_web_queue(&run, &[item]).unwrap();
        state::upsert_task_record(&task_record_fixture(
            "task-auto-recovery",
            "closed:failed",
            &repo,
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let recovered = &stored_items[0];

        assert_eq!(stored_run.status, "running");
        assert_eq!(recovered.status, "pending");
        assert_eq!(recovered.recovery_attempts, 1);
        assert!(recovered.agent_id.is_none());
        assert!(recovered.message.contains("auto-recovery scheduled"));
        assert!(recovered.message.contains("closed:failed"));
        assert!(matches!(
            reconcile_queue_task_statuses(&stored_run, &stored_items).unwrap(),
            QueueReconcile::Unchanged
        ));
        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert_eq!(stored_items[0].status, "pending");
        assert_eq!(stored_items[0].recovery_attempts, 1);
    }

    #[test]
    fn closed_failed_queue_task_after_auto_recovery_remains_failed() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("auto-recovery-exhausted", &repo);
        let mut item = queue_taskflow_item("task-auto-recovery-exhausted", &repo, None);
        item.run_id = run.id.clone();
        item.status = "running".to_string();
        item.agent_id = Some("agent-recovery".to_string());
        item.recovery_attempts = 1;
        state::replace_web_queue(&run, &[item]).unwrap();
        state::upsert_task_record(&task_record_fixture(
            "task-auto-recovery-exhausted",
            "closed:failed",
            &repo,
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Terminal
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let failed = &stored_items[0];

        assert_eq!(stored_run.unwrap().status, "failed");
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.recovery_attempts, 1);
        assert_eq!(failed.message, "closed:failed");
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

    #[test]
    fn remote_agent_contract_gets_selected_launcher_env() {
        let _guard = test_support::env_guard();
        let mut command = std::process::Command::new("cargo");

        set_remote_agent_launcher_env(&mut command, "/tmp/remote-dev-env");

        assert_eq!(
            command_env_value(&command, "QCOLD_REMOTE_DEV_ENV_WRAPPER").as_deref(),
            Some("/tmp/remote-dev-env")
        );
        assert_eq!(
            command_env_value(&command, "VITASTOR_REMOTE_DEV_ENV_WRAPPER").as_deref(),
            None
        );
    }

    #[test]
    fn remote_agent_contract_uses_configured_launcher_env_alias() {
        let _guard = test_support::env_guard();
        std::env::set_var(
            "QCOLD_QUEUE_REMOTE_AGENT_LAUNCHER_ENV",
            "TARGET_REMOTE_DEV_ENV_WRAPPER",
        );
        let mut command = std::process::Command::new("cargo");

        set_remote_agent_launcher_env(&mut command, "/tmp/remote-dev-env");

        assert_eq!(
            command_env_value(&command, "TARGET_REMOTE_DEV_ENV_WRAPPER").as_deref(),
            Some("/tmp/remote-dev-env")
        );
    }

    #[test]
    fn remote_native_launch_failure_uses_retry_failure_state() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("remote-native-retry", &repo);
        let mut item = queue_taskflow_item("task-remote-native-retry", &repo, Some("/bin/false"));
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".to_string();
        item.attempts = WEB_QUEUE_RETRY_DELAYS.len() as i64;
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let outcome = run_web_queue_item(&run.id, &item).unwrap();

        assert!(matches!(
            outcome,
            QueueItemOutcome::Failed {
                retryable: false,
                ..
            }
        ));
        let (_, items) = state::load_web_queue_run(&run.id).unwrap();
        assert_eq!(items[0].status, "failed");
        assert_eq!(items[0].attempts, WEB_QUEUE_RETRY_DELAYS.len() as i64);
        assert!(items[0].next_attempt_at.is_none());
        assert_eq!(items[0].agent_id.as_deref(), Some("qa-task-remote-native-retry"));
        assert!(
            items[0]
                .message
                .contains("repository remote-agent doctor contract failed"),
            "{}",
            items[0].message
        );
    }

    #[test]
    fn remote_native_task_status_propagates_sync_failure() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_taskflow_item("task-remote-native-sync", &repo, Some("/bin/false"));
        item.execution_host = "remote-native".to_string();
        item.status = "running".to_string();
        item.agent_id = Some(queue_agent_id(&item));

        let err = queue_task_status(&item).unwrap_err();

        assert!(
            format!("{err:#}").contains("remote-native task-record sync failed"),
            "{err:#}"
        );
    }

    #[test]
    fn remote_native_launch_wait_item_forces_task_record_sync() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_taskflow_item("task-remote-native-wait", &repo, Some("remote-dev-env"));
        item.execution_host = "remote-native".to_string();

        let wait_item = remote_native_running_wait_item(&item);

        assert!(!remote_native_requires_task_record_sync(&item));
        assert!(remote_native_requires_task_record_sync(&wait_item));
        assert_eq!(wait_item.remote_launcher, item.remote_launcher);
        assert_eq!(wait_item.slug, item.slug);
    }

    #[test]
    fn remote_native_instruction_script_retries_codex_paste_submit() {
        let script = remote_native_instruction_script(
            "session-task-packet",
            "qcold-qa-task-remote-native:0.0",
        );

        let ready_probe = script
            .find("for ready_attempt in 1 2 3")
            .expect("script should wait for Codex readiness");
        let update_skip = script
            .find("Update available!")
            .expect("script should handle the Codex update prompt");
        let paste = script
            .find("tmux paste-buffer -b session-task-packet")
            .expect("script should paste the prompt");

        assert!(ready_probe < paste);
        assert!(update_skip < paste);
        assert!(script.contains("grep -q 'Update now.*@openai/codex'"));
        assert!(script.contains("tmux send-keys -t qcold-qa-task-remote-native:0.0 Down Down C-m"));
        assert!(script.contains("grep -Eq 'OpenAI Codex|^[[:space:]]*"));
        assert!(script.contains("›[^0-9.]"));
        assert!(script.contains("remote-native target did not become ready for Codex input"));
        assert!(script.contains("exit 70"));
        assert!(script.contains("sleep 2"));
        assert!(script.contains("tmux paste-buffer -b session-task-packet"));
        assert!(script.contains("tmux send-keys -t qcold-qa-task-remote-native:0.0 C-m"));
        assert!(script.contains("grep -q '\\[Pasted Content'"));
        assert!(script.contains("for attempt in 1 2 3 4 5 6"));
    }

    #[test]
    fn recovery_task_packet_is_one_shot_and_uses_separate_agent_id() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_taskflow_item("task-auto-recovery-packet", &repo, None);
        let first_agent = queue_agent_id(&item);
        item.recovery_attempts = 1;
        item.message = "closed:failed".to_string();

        let packet = queue_task_instruction(&item);

        assert_ne!(queue_agent_id(&item), first_agent);
        assert!(packet.contains("auto_recovery:"));
        assert!(packet.contains("attempt: 1/1"));
        assert!(packet.contains("make one repair attempt"));
        assert!(packet.contains("previous_failure:"));
        assert!(packet.contains("closed:failed"));
    }

    #[test]
    #[cfg(unix)]
    fn remote_native_cleanup_stops_remote_agent_session() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let log = install_fake_cargo_logger(temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_taskflow_item(
            "task-remote-native-cleanup",
            &repo,
            Some("remote-dev-env"),
        );
        item.execution_host = "remote-native".to_string();
        item.status = "failed".to_string();
        item.agent_id = Some(queue_agent_id(&item));
        let mut task = task_record_fixture("task-remote-native-cleanup", "open", &repo);
        task.agent_id = item.agent_id.clone();
        state::upsert_task_record(&task).unwrap();

        cleanup_queue_item_artifacts(&item, None, None).unwrap();

        assert!(
            state::get_task_record("task/task-remote-native-cleanup")
                .unwrap()
                .is_none()
        );
        let log = fs::read_to_string(log).unwrap();
        assert!(log.contains("xtask remote-agent down --session qcold-qa-task-remote-native-cleanup"));
        assert!(log.contains("QCOLD_REMOTE_DEV_ENV_WRAPPER=remote-dev-env"));
        assert!(!log.contains("remote-agent open"));
    }

    #[test]
    fn remote_native_stopped_item_resumes_without_reopening() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("remote-native-stopped", &repo);
        let mut item = queue_taskflow_item("task-remote-native-stopped", &repo, None);
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".to_string();
        item.status = "stopped".to_string();
        item.agent_id = Some(queue_agent_id(&item));
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let outcome = run_web_queue_item(&run.id, &item).unwrap();

        assert!(matches!(outcome, QueueItemOutcome::Failed { .. }));
        let (_, items) = state::load_web_queue_run(&run.id).unwrap();
        assert_eq!(items[0].status, "failed");
        assert!(
            items[0]
                .message
                .contains("remote-native task record was not visible"),
            "{}",
            items[0].message
        );
        assert!(!items[0].message.contains("requires remote_launcher"));
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
    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    fn queue_run_fixture(id: &str, repo: &Path) -> state::QueueRunRow {
        state::QueueRunRow {
            id: id.to_string(),
            status: "running".to_string(),
            execution_mode: "sequence".to_string(),
            execution_host: "local".to_string(),
            selected_agent_command: "c1".to_string(),
            remote_launcher: None,
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
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
            execution_host: "local".to_string(),
            agent_command: "c1".to_string(),
            remote_launcher: remote_launcher.map(str::to_string),
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            recovery_attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        }
    }

    fn command_env_value(command: &std::process::Command, name: &str) -> Option<String> {
        command.get_envs().find_map(|(key, value)| {
            (key == name).then(|| value.map(|value| value.to_string_lossy().into_owned()))?
        })
    }
}
