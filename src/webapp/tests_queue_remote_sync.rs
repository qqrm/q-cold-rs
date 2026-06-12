#[cfg(test)]
mod queue_remote_sync_tests {
    #![allow(clippy::unwrap_used)]

    use crate::{state, test_support};

    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[cfg(unix)]
    #[test]
    fn remote_native_sync_skips_remote_qcold_overlay_after_adapter_success() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let log = temp.path().join("sync.log");
        let qcold = temp.path().join("qcold");
        fs::write(
            &qcold,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n",
                shell_quote(&log)
            ),
        )
        .unwrap();
        make_executable(&qcold);
        let mut item = queue_item("task-remote-sync-overlay", &repo);
        item.execution_host = "remote-native".into();
        item.status = "running".into();

        sync_remote_queue_task_records_with_executable(&item, true, &qcold).unwrap();

        let lines = fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("task-record sync-remote --via remote-dev-env"));
        assert!(lines[0].contains("--limit 200"));
        assert!(!lines[0].contains("--legacy-remote-qcold"));
    }

    #[cfg(unix)]
    #[test]
    fn remote_native_sync_uses_remote_qcold_overlay_after_adapter_failure() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let log = temp.path().join("sync.log");
        let qcold = temp.path().join("qcold");
        fs::write(
            &qcold,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$*\" >> {}\n\
                 case \"$*\" in\n\
                 *--legacy-remote-qcold*) exit 0 ;;\n\
                 *) exit 1 ;;\n\
                 esac\n",
                shell_quote(&log)
            ),
        )
        .unwrap();
        make_executable(&qcold);
        let mut item = queue_item("task-remote-sync-overlay-fallback", &repo);
        item.execution_host = "remote-native".into();
        item.status = "running".into();

        sync_remote_queue_task_records_with_executable(&item, true, &qcold).unwrap();

        let lines = fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("--limit 200"));
        assert!(!lines[0].contains("--legacy-remote-qcold"));
        assert!(lines[1].contains("--limit 200"));
        assert!(lines[1].contains("--legacy-remote-qcold"));
    }

    #[cfg(unix)]
    #[test]
    fn remote_task_record_sync_uses_outer_timeout_wrapper() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let timeout_log = install_fake_timeout_logger(temp.path());
        let qcold_log = temp.path().join("qcold.log");
        let qcold = fake_qcold_sync_logger(temp.path(), &qcold_log, 0);

        run_remote_queue_task_record_sync(&qcold, &repo, "remote-dev-env", false).unwrap();

        let timeout_log = fs::read_to_string(timeout_log).unwrap();
        assert!(timeout_log.contains("--kill-after 5s 30s"), "{timeout_log}");
        assert!(timeout_log.contains(qcold.to_str().unwrap()), "{timeout_log}");
        assert!(timeout_log.contains("task-record sync-remote --via remote-dev-env"));
        let qcold_log = fs::read_to_string(qcold_log).unwrap();
        assert!(qcold_log.contains(&format!("PWD={}", repo.display())), "{qcold_log}");
        assert!(
            qcold_log.contains(&format!("QCOLD_REPO_ROOT={}", repo.display())),
            "{qcold_log}"
        );
        assert!(qcold_log.contains("ARGS=task-record sync-remote --via remote-dev-env"));
    }

    #[cfg(unix)]
    #[test]
    fn remote_task_record_sync_timeout_env_changes_outer_wrapper_duration() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_REMOTE_TASK_RECORD_SYNC_TIMEOUT_SECONDS", "7");
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let timeout_log = install_fake_timeout_logger(temp.path());
        let qcold_log = temp.path().join("qcold.log");
        let qcold = fake_qcold_sync_logger(temp.path(), &qcold_log, 0);

        run_remote_queue_task_record_sync(&qcold, &repo, "remote-dev-env", true).unwrap();

        let timeout_log = fs::read_to_string(timeout_log).unwrap();
        assert!(timeout_log.contains("--kill-after 5s 7s"), "{timeout_log}");
        assert!(timeout_log.contains("--legacy-remote-qcold"), "{timeout_log}");
    }

    #[cfg(unix)]
    #[test]
    fn remote_native_sync_limit_can_be_lowered_by_env() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        std::env::set_var("QCOLD_QUEUE_REMOTE_TASK_RECORD_SYNC_LIMIT", "20");
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let log = temp.path().join("sync.log");
        let qcold = temp.path().join("qcold");
        fs::write(
            &qcold,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\n",
                shell_quote(&log)
            ),
        )
        .unwrap();
        make_executable(&qcold);
        let mut item = queue_item("task-remote-sync-limit", &repo);
        item.execution_host = "remote-native".into();
        item.status = "running".into();

        sync_remote_queue_task_records_with_executable(&item, true, &qcold).unwrap();

        let output = fs::read_to_string(log).unwrap();
        assert!(output.contains("--limit 20"));
    }


    #[test]
    fn pending_remote_native_without_record_skips_optional_remote_sync() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_item("task-remote-native-future", &repo);
        item.execution_host = "remote-native".into();
        item.status = "pending".into();

        assert!(optional_remote_sync_unneeded(
            &item,
            &QueueTaskLocalStatus::none()
        ));

        item.status = "running".into();
        assert!(!optional_remote_sync_unneeded(
            &item,
            &QueueTaskLocalStatus::none()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn pending_remote_native_open_record_skips_optional_remote_sync() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let sync_attempt = temp.path().join("sync-attempted");
        let remote_launcher = temp.path().join("remote-dev-env");
        fs::write(
            &remote_launcher,
            format!(
                "#!/bin/sh\n\
                 touch {}\n\
                 exit 1\n",
                shell_quote(&sync_attempt),
            ),
        )
        .unwrap();
        make_executable(&remote_launcher);
        let mut item = queue_item("iouring-rust-05-eventfd-docs", &repo);
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some(remote_launcher.display().to_string());
        item.status = "pending".into();
        state::upsert_task_record(&task_record(&item.slug, "open", &repo, Some("abeliakov")))
            .unwrap();

        assert_eq!(queue_task_status(&item).unwrap().as_deref(), Some("open"));
        assert!(
            !sync_attempt.exists(),
            "pending remote-native open record tried optional remote sync"
        );
    }

    #[test]
    fn remote_native_wait_item_keeps_launch_agent_id() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut item = queue_item("task-remote-native-wait-agent", &repo);
        item.execution_host = "remote-native".into();
        item.status = "pending".into();
        item.agent_id = None;

        let wait_item = remote_native_running_wait_item(&item, "qa-task-remote-native-wait-agent");

        assert_eq!(wait_item.status, state::QueueItemStatus::Running);
        assert_eq!(
            wait_item.agent_id.as_deref(),
            Some("qa-task-remote-native-wait-agent")
        );
    }

    #[test]
    fn open_remote_native_record_without_session_relaunches_item() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run("remote-native-disconnected", &repo);
        let mut item = queue_item("task-remote-native-disconnected", &repo);
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some("/bin/false".to_string());
        item.status = "running".into();
        item.agent_id = Some("qa-task-remote-native-disconnected".to_string());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::upsert_task_record(&task_record(
            &item.slug,
            "open",
            &repo,
            item.agent_id.as_deref(),
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let relaunched = &stored_items[0];

        assert_eq!(stored_run.status, "running");
        assert_eq!(
            stored_run.message,
            REMOTE_NATIVE_OPEN_RECORD_RELAUNCH_MESSAGE
        );
        assert_eq!(relaunched.status, "pending");
        assert_eq!(
            relaunched.message,
            REMOTE_NATIVE_OPEN_RECORD_RELAUNCH_MESSAGE
        );
        assert_eq!(relaunched.agent_id.as_deref(), None);
    }

    #[test]
    fn failed_remote_native_sync_row_with_open_record_relaunches_on_continue() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut run = queue_run("remote-native-failed-sync-disconnected", &repo);
        run.status = "failed".into();
        run.current_index = 0;
        run.message = "remote-native task-record sync failed: timeout".to_string();
        let mut item = queue_item("task-remote-native-failed-sync-disconnected", &repo);
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some("/bin/false".to_string());
        item.status = "failed".into();
        item.message = run.message.clone();
        item.agent_id = Some("qa-task-remote-native-failed-sync-disconnected".to_string());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::upsert_task_record(&task_record(
            &item.slug,
            "open",
            &repo,
            item.agent_id.as_deref(),
        ))
        .unwrap();

        assert!(continue_resolved_failed_queue_run(&run.id).unwrap());
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let relaunched = &stored_items[0];

        assert_eq!(stored_run.status, "running");
        assert_eq!(stored_run.current_index, 0);
        assert_eq!(relaunched.status, "pending");
        assert_eq!(relaunched.agent_id.as_deref(), None);
        assert_eq!(
            relaunched.message,
            REMOTE_NATIVE_OPEN_RECORD_RELAUNCH_MESSAGE
        );
    }

    #[test]
    fn waiting_remote_native_record_without_session_relaunches_item() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run("remote-native-waiting-disconnected", &repo);
        let mut item = queue_item("task-remote-native-waiting-disconnected", &repo);
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some("/bin/false".to_string());
        item.status = "waiting".into();
        item.message = "remote-agent open retry scheduled".to_string();
        item.attempts = 1;
        item.next_attempt_at = Some(1);
        item.agent_id = Some("qa-task-remote-native-waiting-disconnected".to_string());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::upsert_task_record(&task_record(
            &item.slug,
            "open",
            &repo,
            item.agent_id.as_deref(),
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let relaunched = &stored_items[0];

        assert_eq!(stored_run.status, "running");
        assert_eq!(stored_run.current_index, 0);
        assert_eq!(relaunched.status, "pending");
        assert_eq!(relaunched.attempts, 1);
        assert_eq!(relaunched.next_attempt_at, None);
        assert_eq!(
            relaunched.message,
            REMOTE_NATIVE_OPEN_RECORD_RELAUNCH_MESSAGE
        );
        assert_eq!(relaunched.agent_id.as_deref(), None);
    }

    #[test]
    fn stale_stopped_remote_native_record_without_session_relaunches_item() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut run = queue_run("remote-native-stopped-disconnected", &repo);
        run.status = "running".into();
        run.current_index = 0;
        let mut item = queue_item("task-remote-native-stopped-disconnected", &repo);
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some("/bin/false".to_string());
        item.status = "stopped".into();
        item.message = REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE.to_string();
        item.agent_id = Some("qa-task-remote-native-stopped-disconnected".to_string());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::upsert_task_record(&task_record(
            &item.slug,
            "open",
            &repo,
            item.agent_id.as_deref(),
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let relaunched = &stored_items[0];

        assert_eq!(stored_run.status, "running");
        assert_eq!(stored_run.current_index, 0);
        assert_eq!(relaunched.status, "pending");
        assert_eq!(relaunched.agent_id.as_deref(), None);
        assert_eq!(
            relaunched.message,
            REMOTE_NATIVE_OPEN_RECORD_RELAUNCH_MESSAGE
        );
    }

    #[cfg(unix)]
    #[test]
    fn stopped_remote_native_open_record_with_live_session_resumes_running() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let remote_launcher = temp.path().join("remote-dev-env");
        fs::write(&remote_launcher, "#!/bin/sh\nexit 0\n").unwrap();
        make_executable(&remote_launcher);
        let mut run = queue_run("remote-native-live-stopped", &repo);
        run.status = "stopped".into();
        run.current_index = 0;
        run.message = REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE.to_string();
        let mut item = queue_item("task-remote-native-live-stopped", &repo);
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some(remote_launcher.display().to_string());
        item.status = "stopped".into();
        item.message = REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE.to_string();
        item.agent_id = Some("qa-task-remote-native-live-stopped".to_string());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::upsert_task_record(&task_record(
            &item.slug,
            "open",
            &repo,
            item.agent_id.as_deref(),
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let resumed = &stored_items[0];

        assert_eq!(stored_run.status, "running");
        assert_eq!(stored_run.message, "running");
        assert_eq!(resumed.status, "running");
        assert_eq!(
            resumed.agent_id.as_deref(),
            Some("qa-task-remote-native-live-stopped")
        );
        assert!(
            resumed
                .message
                .contains("resumed remote-native agent qa-task-remote-native-live-stopped"),
            "{}",
            resumed.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn stopped_remote_native_open_record_with_relaunch_session_resumes_running() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let remote_launcher = temp.path().join("remote-dev-env");
        fs::write(
            &remote_launcher,
            "#!/bin/sh\ncase \"$*\" in *qcold-qa-task-remote-native-live-relaunch*) exit 0;; *) exit 1;; esac\n",
        )
        .unwrap();
        make_executable(&remote_launcher);
        let mut run = queue_run("remote-native-relaunch-stopped", &repo);
        run.status = "stopped".into();
        run.current_index = 0;
        run.message = REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE.to_string();
        let mut item = queue_item("task-remote-native-live-relaunch", &repo);
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some(remote_launcher.display().to_string());
        item.status = "stopped".into();
        item.message = REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE.to_string();
        item.agent_id = Some("qa-task-remote-native-live-deadbeef".to_string());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::upsert_task_record(&task_record(
            &item.slug,
            "open",
            &repo,
            item.agent_id.as_deref(),
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let resumed = &stored_items[0];

        assert_eq!(stored_run.status, "running");
        assert_eq!(resumed.status, "running");
        assert_eq!(
            resumed.agent_id.as_deref(),
            Some("qa-task-remote-native-live-relaunch")
        );
        assert!(
            resumed
                .message
                .contains("resumed remote-native agent qa-task-remote-native-live-relaunch"),
            "{}",
            resumed.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn stopped_remote_native_open_record_with_repair_session_resumes_running() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let remote_launcher = temp.path().join("remote-dev-env");
        fs::write(
            &remote_launcher,
            "#!/bin/sh\n\
             case \"$*\" in *qcold-qa-task-remote-native-live-repair*) exit 0;; *) exit 1;; esac\n",
        )
        .unwrap();
        make_executable(&remote_launcher);
        let mut run = queue_run("remote-native-repair-stopped", &repo);
        run.status = "stopped".into();
        run.current_index = 0;
        run.message = REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE.to_string();
        let mut item = queue_item("task-remote-native-live-repair", &repo);
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some(remote_launcher.display().to_string());
        item.status = "stopped".into();
        item.message = REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE.to_string();
        item.agent_id = Some("qa-task-remote-native-live-deadbeef".to_string());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::upsert_task_record(&task_record(
            &item.slug,
            "open",
            &repo,
            item.agent_id.as_deref(),
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let resumed = &stored_items[0];

        assert_eq!(stored_run.status, "running");
        assert_eq!(resumed.status, "running");
        assert_eq!(
            resumed.agent_id.as_deref(),
            Some("qa-task-remote-native-live-repair")
        );
        assert!(
            resumed
                .message
                .contains("resumed remote-native agent qa-task-remote-native-live-repair"),
            "{}",
            resumed.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn stopped_remote_native_open_record_with_numbered_repair_session_resumes_running() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let remote_launcher = temp.path().join("remote-dev-env");
        fs::write(
            &remote_launcher,
            "#!/bin/sh\n\
             case \"$*\" in\n\
             *=qcold-qa-task-remote-native-live-repair2*) exit 0 ;;\n\
             *=qcold-qa-task-remote-native-live-repair*) exit 1 ;;\n\
             *qcold-qa-task-remote-native-live-repair*) exit 0 ;;\n\
             *) exit 1 ;;\n\
             esac\n",
        )
        .unwrap();
        make_executable(&remote_launcher);
        let mut run = queue_run("remote-native-numbered-repair-stopped", &repo);
        run.status = "stopped".into();
        run.current_index = 0;
        run.message = REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE.to_string();
        let mut item = queue_item("task-remote-native-live-repair2", &repo);
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".into();
        item.remote_launcher = Some(remote_launcher.display().to_string());
        item.status = "stopped".into();
        item.message = REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE.to_string();
        item.agent_id = Some("qa-task-remote-native-live-repair".to_string());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::upsert_task_record(&task_record(
            &item.slug,
            "open",
            &repo,
            item.agent_id.as_deref(),
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let resumed = &stored_items[0];

        assert_eq!(stored_run.status, "running");
        assert_eq!(resumed.status, "running");
        assert_eq!(
            resumed.agent_id.as_deref(),
            Some("qa-task-remote-native-live-repair2")
        );
        assert!(
            resumed
                .message
                .contains("resumed remote-native agent qa-task-remote-native-live-repair2"),
            "{}",
            resumed.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn remote_native_running_sequence_accepts_imported_success_without_resync() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let sync_attempt = temp.path().join("sync-attempted");
        let remote_launcher = temp.path().join("remote-dev-env");
        fs::write(
            &remote_launcher,
            format!(
                "#!/bin/sh\n\
                 touch {}\n\
                 exit 1\n",
                shell_quote(&sync_attempt),
            ),
        )
        .unwrap();
        make_executable(&remote_launcher);
        let run = queue_run("remote-native-imported-success-sequence", &repo);
        let mut first = queue_item("iouring-rust-03-fd-semantics", &repo);
        first.run_id = run.id.clone();
        first.id = "03-fd-semantics".to_string();
        first.position = 3;
        first.execution_host = "remote-native".into();
        first.remote_launcher = Some(remote_launcher.display().to_string());
        first.status = "running".into();
        first.agent_id = Some("qa-iouring-rust-03-fd-semantics".to_string());
        let mut second = queue_item("iouring-rust-04-eager-executor", &repo);
        second.run_id = run.id.clone();
        second.id = "04-eager-executor".to_string();
        second.position = 4;
        state::replace_web_queue(&run, &[first, second]).unwrap();
        state::upsert_task_record(&state::new_task_record(
            "task/iouring-rust-03-fd-semantics".to_string(),
            "task-flow".to_string(),
            "Iouring Rust 03 Fd Semantics".to_string(),
            "Remote work".to_string(),
            "closed:success".to_string(),
            Some(repo.display().to_string()),
            Some("/home/abeliakov/vitastor".to_string()),
            Some("abeliakov".to_string()),
            None,
        ))
        .unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();

        assert!(!sync_attempt.exists(), "queue tried remote sync despite imported success");
        assert_eq!(
            stored_items
                .iter()
                .map(|item| (item.id.as_str(), item.status.as_str()))
                .collect::<Vec<_>>(),
            [("03-fd-semantics", "success"), ("04-eager-executor", "pending")]
        );
        assert_eq!(
            queue_ready_items(&stored_run, &stored_items)
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["04-eager-executor"]
        );
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

    #[cfg(unix)]
    fn install_fake_timeout_logger(temp: &Path) -> PathBuf {
        let bin = temp.join("timeout-bin");
        fs::create_dir(&bin).unwrap();
        let log = temp.join("timeout.log");
        let timeout = bin.join("timeout");
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\n' \"$*\" >> {}\n\
             if [ \"$1\" = '--kill-after' ]; then\n\
             shift 3\n\
             else\n\
             shift\n\
             fi\n\
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
    fn fake_qcold_sync_logger(temp: &Path, log: &Path, exit_code: i32) -> PathBuf {
        let qcold = temp.join("qcold");
        let script = format!(
            "#!/bin/sh\n\
             printf 'PWD=%s QCOLD_REPO_ROOT=%s ARGS=%s\n' \"$PWD\" \"$QCOLD_REPO_ROOT\" \"$*\" >> {}\n\
             exit {exit_code}\n",
            shell_quote(log)
        );
        fs::write(&qcold, script).unwrap();
        make_executable(&qcold);
        qcold
    }

    fn queue_run(id: &str, repo: &Path) -> state::QueueRunRow {
        state::QueueRunRow {
            id: id.to_string(),
            status: "running".into(),
            execution_mode: "sequence".into(),
            execution_host: "remote-native".into(),
            selected_agent_command: "c1".to_string(),
            remote_launcher: Some("remote-dev-env".to_string()),
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            selected_repo_root: Some(repo.display().to_string()),
            selected_repo_name: Some("repo".to_string()),
            track: format!("queue-{id}"),
            current_index: -1,
            stop_requested: false,
            message: "queued".to_string(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn task_record(
        slug: &str,
        status: &str,
        repo: &Path,
        agent_id: Option<&str>,
    ) -> state::TaskRecordRow {
        state::new_task_record(
            format!("task/{slug}"),
            "task-flow".to_string(),
            slug.to_string(),
            "existing task".to_string(),
            status.to_string(),
            Some(repo.display().to_string()),
            Some(repo.join("WT").join(slug).display().to_string()),
            agent_id.map(str::to_string),
            None,
        )
    }

    fn queue_item(slug: &str, repo: &Path) -> state::QueueItemRow {
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
            remote_launcher: Some("remote-dev-env".to_string()),
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
}
