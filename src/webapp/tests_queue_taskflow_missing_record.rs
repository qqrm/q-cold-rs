#[cfg(test)]
mod queue_taskflow_missing_record_tests {
    #![allow(clippy::unwrap_used)]

    use crate::test_support;

    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn remote_native_missing_task_record_without_live_session_fails_even_with_launcher() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("remote-native-missing-record", &repo);
        let mut item = queue_taskflow_item(
            "task-remote-native-missing-record",
            &repo,
            Some("/bin/false"),
        );
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".to_string();
        item.status = "running".to_string();
        let agent_id = queue_agent_id(&item);
        item.agent_id = Some(agent_id.clone());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let outcome =
            missing_queue_task_record_outcome(&run.id, &item, &agent_id, item.attempts).unwrap();

        assert!(matches!(
            outcome,
            Some(QueueItemOutcome::Failed {
                retryable: true,
                ..
            })
        ));
        let (_, items) = state::load_web_queue_run(&run.id).unwrap();
        assert_eq!(items[0].status, "failed");
        assert_eq!(items[0].agent_id.as_deref(), Some(agent_id.as_str()));
        assert!(
            items[0]
                .message
                .contains("remote-native task record was not visible")
        );
    }

    #[test]
    #[cfg(unix)]
    fn remote_native_missing_task_record_with_live_session_keeps_waiting() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("remote-native-live-missing-record", &repo);
        let mut item = queue_taskflow_item(
            "task-remote-native-live-missing-record",
            &repo,
            Some("/bin/true"),
        );
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".to_string();
        item.status = "running".to_string();
        let agent_id = queue_agent_id(&item);
        item.agent_id = Some(agent_id.clone());
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let outcome =
            missing_queue_task_record_outcome(&run.id, &item, &agent_id, item.attempts).unwrap();

        assert!(outcome.is_none());
        let (_, items) = state::load_web_queue_run(&run.id).unwrap();
        assert_eq!(items[0].status, "running");
        assert_eq!(items[0].agent_id.as_deref(), Some(agent_id.as_str()));
        assert!(items[0].message.contains("task record visibility"));
    }

    #[test]
    #[cfg(unix)]
    fn remote_native_open_record_refreshes_stale_visibility_message() {
        let _guard = test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let run = queue_run_fixture("remote-native-sync", &repo);
        let mut item = queue_taskflow_item("task-remote-native-sync", &repo, Some("/bin/true"));
        item.run_id = run.id.clone();
        item.execution_host = "remote-native".to_string();
        item.status = "running".to_string();
        item.message =
            "waiting for remote-native task record visibility after remote-agent open".to_string();
        item.agent_id = Some(queue_agent_id(&item));
        let mut record = task_record_fixture("task-remote-native-sync", "open", &repo);
        record.agent_id.clone_from(&item.agent_id);
        state::replace_web_queue(&run, &[item.clone()]).unwrap();
        state::upsert_task_record(&record).unwrap();

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        assert!(matches!(
            reconcile_queue_task_statuses(&run, &stored_items).unwrap(),
            QueueReconcile::Changed
        ));

        let (_, stored_items) = state::load_web_queue_run(&run.id).unwrap();
        let stored = &stored_items[0];
        let agent_id = item.agent_id.as_deref().unwrap();
        assert_eq!(stored.status, "running");
        assert_eq!(stored.message, format!("repo {} ({agent_id})", item.slug));
    }

    fn queue_run_fixture(id: &str, repo: &Path) -> state::QueueRunRow {
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
            track: "queue-run".to_string(),
            current_index: -1,
            stop_requested: false,
            message: "queued".to_string(),
            created_at: 0,
            updated_at: 0,
        }
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
}
