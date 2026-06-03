#[cfg(test)]
mod queue_remote_sync_tests {
    #![allow(clippy::unwrap_used)]

    use crate::{state, test_support};

    use super::*;
    use std::fs;
    use std::path::Path;

    #[cfg(unix)]
    #[test]
    fn remote_native_sync_adds_remote_qcold_overlay() {
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
        item.execution_host = "remote-native".to_string();
        item.status = "running".to_string();

        sync_remote_queue_task_records_with_executable(&item, true, &qcold).unwrap();

        let lines = fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("task-record sync-remote --via remote-dev-env"));
        assert!(!lines[0].contains("--legacy-remote-qcold"));
        assert!(lines[1].contains("task-record sync-remote --via remote-dev-env"));
        assert!(lines[1].contains("--legacy-remote-qcold"));
    }

    #[test]
    fn open_remote_native_record_without_session_marks_queue_stopped() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run("remote-native-disconnected", &repo);
        let mut item = queue_item("task-remote-native-disconnected", &repo);
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".to_string();
        item.remote_launcher = Some("/bin/false".to_string());
        item.status = "running".to_string();
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
            QueueReconcile::Terminal
        ));
        let (stored_run, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored_run = stored_run.unwrap();
        let stopped = &stored_items[0];

        assert_eq!(stored_run.status, "stopped");
        assert_eq!(stored_run.message, REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE);
        assert_eq!(stopped.status, "stopped");
        assert_eq!(stopped.message, REMOTE_NATIVE_DISCONNECTED_OPEN_MESSAGE);
        assert_eq!(
            stopped.agent_id.as_deref(),
            Some("qa-task-remote-native-disconnected")
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

    fn queue_run(id: &str, repo: &Path) -> state::QueueRunRow {
        state::QueueRunRow {
            id: id.to_string(),
            status: "running".to_string(),
            execution_mode: "sequence".to_string(),
            execution_host: "remote-native".to_string(),
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
            execution_host: "local".to_string(),
            agent_command: "c1".to_string(),
            remote_launcher: Some("remote-dev-env".to_string()),
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
}
