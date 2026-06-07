#[cfg(test)]
mod queue_taskflow_tests {
    use crate::test_support;

    use super::*;

    include!("tests_queue_taskflow_attempts.rs");

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
    fn queue_task_open_skips_cargo_test_harness_executable() {
        let temp = tempfile::tempdir().unwrap();
        let deps = temp.path().join("debug").join("deps");
        fs::create_dir_all(&deps).unwrap();
        let harness = deps.join(format!("qcold-deadbeef{}", env::consts::EXE_SUFFIX));
        fs::write(&harness, "").unwrap();
        make_executable(&harness);
        let installed = temp
            .path()
            .join("debug")
            .join(format!("qcold{}", env::consts::EXE_SUFFIX));
        fs::write(&installed, "").unwrap();
        make_executable(&installed);

        let resolved = queue_qcold_executable_from(&harness, None).unwrap();

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
    fn failed_closeout_queue_task_marks_running_item_failed() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("failed-closeout", &repo);
        let mut item = queue_taskflow_item("task-failed-closeout", &repo, None);
        item.run_id = run.id.clone();
        item.status = "running".into();
        item.execution_host = "remote-native".into();
        item.agent_id = Some("qa-task-failed-closeout".to_string());
        state::replace_web_queue(&run, &[item]).unwrap();
        state::upsert_task_record(&task_record_fixture(
            "task-failed-closeout",
            "failed-closeout",
            &repo,
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Terminal
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let failed = &stored_items[0];

        assert_eq!(stored_run.status, "failed");
        assert_eq!(stored_run.message, "failed-closeout");
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.message, "failed-closeout");
        assert_eq!(failed.agent_id.as_deref(), Some("qa-task-failed-closeout"));
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
        existing.status = "running".into();
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
        item.execution_host = "remote-native".into();
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
    fn retryable_remote_native_port_forward_failure_rotates_proxy() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("remote-native-port-rotate", &repo);
        let mut item = queue_taskflow_item("task-remote-native-port-rotate", &repo, Some("remote-dev-env"));
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.remote_agent_remote_proxy = Some("127.0.0.1:18330".to_string());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let rotated = maybe_rotate_remote_native_proxy_after_failure(
            &mut item,
            concat!(
                "repository remote-agent open contract failed with exit status: 255: ",
                "Error: remote port forwarding failed for listen port 18330",
            ),
        )
        .unwrap();

        assert_eq!(rotated.as_deref(), Some("127.0.0.1:18331"));
        assert_eq!(
            item.remote_agent_remote_proxy.as_deref(),
            Some("127.0.0.1:18331")
        );
        let (_, items) = state::load_web_queue_run(&run.id).unwrap();
        assert_eq!(
            items[0].remote_agent_remote_proxy.as_deref(),
            Some("127.0.0.1:18331")
        );
    }

    #[test]
    fn remote_native_launch_reserves_new_proxy_when_failed_row_already_owns_port() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("remote-native-port-reserve", &repo);
        let mut failed = queue_taskflow_item("task-remote-native-failed", &repo, Some("remote-dev-env"));
        failed.run_id = run.id.clone();
        failed.id = "failed".into();
        failed.execution_host = "remote-native".into();
        failed.status = "failed".into();
        failed.remote_agent_remote_proxy = Some("127.0.0.1:18330".to_string());
        let mut pending = queue_taskflow_item("task-remote-native-pending", &repo, Some("remote-dev-env"));
        pending.run_id = run.id.clone();
        pending.id = "pending".into();
        pending.execution_host = "remote-native".into();
        pending.position = 1;
        pending.remote_agent_remote_proxy = Some("127.0.0.1:18330".to_string());
        state::replace_web_queue(&run, &[failed, pending.clone()]).unwrap();

        let rotated = reserve_remote_native_proxy(&mut pending).unwrap();

        assert_eq!(rotated.as_deref(), Some("127.0.0.1:18331"));
        assert_eq!(
            pending.remote_agent_remote_proxy.as_deref(),
            Some("127.0.0.1:18331")
        );
        let (_, items) = state::load_web_queue_run(&run.id).unwrap();
        let stored = items.iter().find(|item| item.id == "pending").unwrap();
        assert_eq!(
            stored.remote_agent_remote_proxy.as_deref(),
            Some("127.0.0.1:18331")
        );
    }

    #[test]
    fn remote_native_task_status_uses_open_record_on_sync_failure() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_taskflow_item("task-remote-native-sync", &repo, Some("/bin/false"));
        item.execution_host = "remote-native".into();
        item.status = "running".into();
        item.agent_id = Some(queue_agent_id(&item));
        let mut record = task_record_fixture("task-remote-native-sync", "open", &repo);
        record.agent_id.clone_from(&item.agent_id);
        state::upsert_task_record(&record).unwrap();

        let status = queue_task_status(&item).unwrap();

        assert_eq!(status.as_deref(), Some("open"));
    }

    #[test]
    fn required_remote_queue_sync_is_throttled_per_launcher_and_repo() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let item = queue_taskflow_item("task-remote-sync-throttle", &repo, Some("remote-dev-env"));

        assert!(remote_queue_sync_due(&item, "remote-dev-env", true));
        assert!(!remote_queue_sync_due(&item, "remote-dev-env", true));
    }

    #[test]
    fn queue_task_status_prefers_newer_recovery_record() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_taskflow_item("task-recovery-link", &repo, None);
        item.started_at = 100;
        let mut blocked = task_record_fixture("task-recovery-link", "closed:blocked", &repo);
        blocked.updated_at = 110;
        let mut recovery =
            task_record_fixture("task-recovery-link-recovery", "closed:success", &repo);
        recovery.updated_at = 120;
        state::upsert_task_record(&blocked).unwrap();
        state::upsert_task_record(&recovery).unwrap();

        let status = queue_task_status(&item).unwrap();

        assert_eq!(status.as_deref(), Some("closed:success"));
    }

    #[test]
    fn remote_native_task_status_ignores_stale_failed_record_during_recovery_sync_failure() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_taskflow_item("task-remote-native-sync", &repo, Some("/bin/false"));
        item.execution_host = "remote-native".into();
        item.status = "running".into();
        item.recovery_attempts = 1;
        item.agent_id = Some(queue_agent_id(&item));
        let mut record = task_record_fixture("task-remote-native-sync", "closed:failed", &repo);
        record.agent_id.clone_from(&item.agent_id);
        state::upsert_task_record(&record).unwrap();

        let status = queue_task_status(&item).unwrap();

        assert_eq!(status, None);
    }

    #[test]
    fn remote_native_task_status_propagates_sync_failure_without_local_record() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_taskflow_item("task-remote-native-sync", &repo, Some("/bin/false"));
        item.execution_host = "remote-native".into();
        item.status = "running".into();
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
        item.execution_host = "remote-native".into();

        let wait_item = remote_native_running_wait_item(&item);

        assert!(!remote_native_requires_task_record_sync(&item));
        assert!(remote_native_requires_task_record_sync(&wait_item));
        assert_eq!(wait_item.remote_launcher, item.remote_launcher);
        assert_eq!(wait_item.slug, item.slug);
    }

    #[test]
    #[cfg(unix)]
    fn remote_native_open_passes_prompt_file_to_remote_agent_contract() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let log = install_fake_cargo_logger(temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_taskflow_item(
            "task-remote-native-prompt-file",
            &repo,
            Some("remote-dev-env"),
        );
        item.execution_host = "remote-native".into();
        let prompt_file = write_remote_native_task_packet_file(&item).unwrap();

        run_remote_agent_contract(
            &item,
            &repo,
            "open",
            "qcold-qa-task-remote-native-prompt-file",
            Some(&item.slug),
            Some(&prompt_file),
        )
        .unwrap();

        let log = fs::read_to_string(log).unwrap();
        assert!(log.contains("xtask remote-agent open"));
        assert!(log.contains("--prompt-file"));
        assert!(log.contains(prompt_file.to_str().unwrap()));
        assert!(log.contains("task-remote-native-prompt-file"));
        assert!(log.contains("QCOLD_REMOTE_DEV_ENV_WRAPPER=remote-dev-env"));
        assert!(!log.contains("tmux paste-buffer"));
    }

    #[test]
    #[cfg(unix)]
    fn remote_native_terminal_snapshot_captures_remote_output() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let log = temp.path().join("remote.log");
        std::env::set_var("REMOTE_TERMINAL_LOG", &log);
        let launcher = fake_remote_terminal_launcher(temp.path());
        let timeout_log = install_fake_timeout_logger(temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("remote-native-terminal", &repo);
        let mut item = queue_taskflow_item(
            "task-remote-native-terminal",
            &repo,
            Some(launcher.to_str().unwrap()),
        );
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.status = "running".into();
        item.agent_id = Some(queue_agent_id(&item));
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let panes = discover_remote_native_terminal_sessions(
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
        );

        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].agent_id, item.agent_id.clone().unwrap());
        assert_eq!(
            panes[0].target,
            remote_native_terminal_target(item.agent_id.as_deref().unwrap())
        );
        assert_eq!(panes[0].output, "remote output");
        let log = fs::read_to_string(log).unwrap();
        assert!(log.contains("tmux capture-pane"));
        assert!(log.contains("qcold-qa-task-remote-native-terminal:0.0"));
        let timeout_log = fs::read_to_string(timeout_log).unwrap();
        assert!(timeout_log.contains("20s"));
        assert!(timeout_log.contains(launcher.to_str().unwrap()));
        assert!(timeout_log.contains("tmux capture-pane"));
    }

    #[test]
    #[cfg(unix)]
    fn remote_native_terminal_send_routes_through_remote_launcher() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let log = temp.path().join("remote.log");
        std::env::set_var("REMOTE_TERMINAL_LOG", &log);
        let launcher = fake_remote_terminal_launcher(temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("remote-native-terminal-send", &repo);
        let mut item = queue_taskflow_item(
            "task-remote-native-terminal-send",
            &repo,
            Some(launcher.to_str().unwrap()),
        );
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.status = "running".into();
        item.agent_id = Some(queue_agent_id(&item));
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        let target = remote_native_terminal_target(item.agent_id.as_deref().unwrap());

        send_terminal_key(&target, TerminalKey::Enter).unwrap();

        let log = fs::read_to_string(log).unwrap();
        assert!(log.contains("tmux send-keys"));
        assert!(log.contains("qcold-qa-task-remote-native-terminal-send:0.0"));
        assert!(log.contains("C-m"));
    }

    #[test]
    #[cfg(unix)]
    fn remote_native_terminal_paste_uses_bracketed_raw_tmux_paste() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let log = temp.path().join("remote.log");
        std::env::set_var("REMOTE_TERMINAL_LOG", &log);
        let launcher = fake_remote_terminal_launcher(temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("remote-native-terminal-paste", &repo);
        let mut item = queue_taskflow_item(
            "task-remote-native-terminal-paste",
            &repo,
            Some(launcher.to_str().unwrap()),
        );
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.status = "running".into();
        item.agent_id = Some(queue_agent_id(&item));
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        let target = remote_native_terminal_target(item.agent_id.as_deref().unwrap());

        send_terminal_paste(&target, "line one\nline two", true).unwrap();

        let log = fs::read_to_string(log).unwrap();
        assert!(log.contains("tmux load-buffer -b qcold-web-send-"));
        assert!(log.contains("tmux paste-buffer -d -p -r -b qcold-web-send-"));
        assert!(log.contains("qcold-qa-task-remote-native-terminal-paste:0.0"));
        assert!(log.contains("tmux send-keys"));
        assert!(log.contains("C-m"));
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
        assert!(packet.contains("attempt: 1/2"));
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
        item.execution_host = "remote-native".into();
        item.status = "failed".into();
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
        item.execution_host = "remote-native".into();
        item.status = "stopped".into();
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
    fn fake_remote_terminal_launcher(dir: &Path) -> PathBuf {
        let launcher = dir.join("remote-dev-env");
        fs::write(
            &launcher,
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> \"$REMOTE_TERMINAL_LOG\"\n\
             if [ \"$1\" = tmux ] && [ \"$2\" = capture-pane ]; then\n\
             printf 'remote output\\n'\n\
             elif [ \"$1\" = tmux ] && [ \"$2\" = load-buffer ]; then\n\
             cat >/dev/null\n\
             fi\n",
        )
        .unwrap();
        make_executable(&launcher);
        launcher
    }

    #[cfg(unix)]
    fn install_fake_timeout_logger(temp: &Path) -> PathBuf {
        let bin = temp.join("timeout-bin");
        fs::create_dir(&bin).unwrap();
        let log = temp.join("timeout.log");
        let timeout = bin.join("timeout");
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> {}\n\
             shift\n\
             exec \"$@\"\n",
            shell_quote(&log)
        );
        fs::write(&timeout, script).unwrap();
        make_executable(&timeout);

        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&path));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        log
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
    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    fn queue_run_fixture(id: &str, repo: &Path) -> state::QueueRunRow {
        state::QueueRunRow {
            id: id.to_string(),
            status: "running".into(),
            execution_mode: "sequence".into(),
            execution_host: "local".into(),
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
            execution_host: "local".into(),
            agent_command: "c1".to_string(),
            remote_launcher: remote_launcher.map(str::to_string),
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

    fn command_env_value(command: &std::process::Command, name: &str) -> Option<String> {
        command.get_envs().find_map(|(key, value)| {
            (key == name).then(|| value.map(|value| value.to_string_lossy().into_owned()))?
        })
    }
}
