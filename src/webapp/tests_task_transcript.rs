#[cfg(test)]
mod task_transcript_tests {
    #![allow(clippy::unwrap_used)]

    use std::fs;

    use crate::{state, test_support};

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn task_transcript_refuses_live_web_queue_task_until_closed() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        state::upsert_task_record(&state::TaskRecordRow {
            id: "task/example".to_string(),
            source: "task-flow".to_string(),
            sequence: Some(7),
            title: "example".to_string(),
            description: "operator body".to_string(),
            status: "open".to_string(),
            created_at: 1,
            updated_at: 2,
            repo_root: None,
            cwd: None,
            agent_id: Some("qa-example".to_string()),
            metadata_json: Some(
                serde_json::json!({
                    "opened_by": "web-queue",
                    "session_path": "/tmp/creator-session.jsonl",
                })
                .to_string(),
            ),
        })
        .unwrap();

        let response = task_transcript_response("task/example");

        assert!(!response.ok);
        assert!(response.output.contains("executor terminal only"));
    }

    #[test]
    fn task_transcript_reads_live_task_execution_log_without_codex_session() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let worktree = temp.path().join("WT/qcold/007-live-task");
        let logs = worktree.join(".task/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(
            logs.join("agent-execution.md"),
            "# Agent execution log\n\nstate=empty\nreason=no_matching_rollouts\n",
        )
        .unwrap();
        state::upsert_task_record(&state::TaskRecordRow {
            id: "task/live-task".to_string(),
            source: "task-flow".to_string(),
            sequence: Some(7),
            title: "live task".to_string(),
            description: "operator body".to_string(),
            status: "open".to_string(),
            created_at: 1,
            updated_at: 2,
            repo_root: Some(temp.path().join("repo").display().to_string()),
            cwd: Some(worktree.display().to_string()),
            agent_id: Some("qa-live-task".to_string()),
            metadata_json: Some(
                serde_json::json!({
                    "opened_by": "web-queue",
                    "task_worktree": worktree.display().to_string(),
                })
                .to_string(),
            ),
        })
        .unwrap();

        let response = task_transcript_response("task/live-task");

        assert!(response.ok, "{}", response.output);
        assert_eq!(response.session_path, None);
        assert!(response
            .transcript_path
            .as_deref()
            .is_some_and(|path| path.ends_with(".task/logs/agent-execution.md")));
        assert_eq!(response.messages.len(), 1);
        assert!(response.messages[0].text.contains("reason=no_matching_rollouts"));
    }

    #[test]
    fn task_transcript_for_queued_item_without_record_reports_queue_state_and_spawns_worker() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("run-frontend-freshness");
        let mut item = queue_item_fixture(&run.id, "frontend-freshness");
        item.status = "waiting".into();
        item.message = "admission waiting: available memory is below requested reservation".into();
        item.next_attempt_at = Some(123);
        state::replace_web_queue(&run, &[item]).unwrap();

        let response = task_transcript_response("task/frontend-freshness");

        assert!(response.ok, "{}", response.output);
        assert_eq!(response.task_id, "task/frontend-freshness");
        assert_eq!(response.title, "frontend-freshness");
        assert_eq!(response.status, "waiting");
        assert_eq!(response.session_path, None);
        assert_eq!(response.transcript_path, None);
        assert!(!response.chat_available);
        assert!(response
            .output
            .contains("queued task has not opened a task record yet"));
        assert!(response.output.contains("admission waiting"));
        assert!(response.output.contains("retry_at=123"));
        assert_eq!(response.messages.len(), 1);
        assert!(response.messages[0].text.contains("admission waiting"));
        assert!(test_web_queue_worker_spawned(&run.id));
    }

    #[test]
    fn task_chat_target_for_queued_item_without_record_spawns_worker_instead_of_unknown_record() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("run-chat-frontend-freshness");
        let mut item = queue_item_fixture(&run.id, "frontend-freshness");
        item.status = "waiting".into();
        item.message = "admission waiting: available memory is below requested reservation".into();
        state::replace_web_queue(&run, &[item]).unwrap();

        let err = ensure_task_chat_target("task/frontend-freshness").unwrap_err();
        let output = format!("{err:#}");

        assert!(
            output.contains("queued task has not opened a task record yet"),
            "{output}"
        );
        assert!(!output.contains("unknown task record"));
        assert!(test_web_queue_worker_spawned(&run.id));
    }

    #[test]
    fn task_transcript_for_stopped_queued_item_without_record_does_not_spawn_worker() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut run = queue_run_fixture("run-stopped-frontend-freshness");
        run.status = "stopped".into();
        let mut item = queue_item_fixture(&run.id, "stopped-frontend-freshness");
        item.status = "stopped".into();
        item.message = "stopped by operator; press Continue to resume".into();
        state::replace_web_queue(&run, &[item]).unwrap();

        let response = task_transcript_response("task/stopped-frontend-freshness");

        assert!(response.ok, "{}", response.output);
        assert_eq!(response.status, "stopped");
        assert!(response.output.contains("stopped by operator"));
        assert!(!test_web_queue_worker_spawned(&run.id));
    }

    #[test]
    fn task_chat_target_for_paused_queued_item_without_record_does_not_spawn_worker() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("run-paused-frontend-freshness");
        let mut item = queue_item_fixture(&run.id, "paused-frontend-freshness");
        item.status = "paused".into();
        item.message = "paused by task record status".into();
        state::replace_web_queue(&run, &[item]).unwrap();

        let err = ensure_task_chat_target("task/paused-frontend-freshness").unwrap_err();
        let output = format!("{err:#}");

        assert!(
            output.contains("queued task has not opened a task record yet"),
            "{output}"
        );
        assert!(output.contains("paused by task record status"));
        assert!(!test_web_queue_worker_spawned(&run.id));
    }

    #[test]
    fn task_transcript_for_open_record_without_terminal_reports_queue_state_and_spawns_worker() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("run-eventfd-docs");
        let mut item = queue_item_fixture(&run.id, "iouring-rust-05-eventfd-docs");
        item.status = "waiting".into();
        item.message =
            "admission waiting: available memory is below requested reservation; retry_at=123"
                .into();
        item.next_attempt_at = Some(123);
        state::replace_web_queue(&run, &[item]).unwrap();
        state::upsert_task_record(&task_record_fixture(
            "task/iouring-rust-05-eventfd-docs",
            "open",
        ))
        .unwrap();

        let response = task_transcript_response("task/iouring-rust-05-eventfd-docs");

        assert!(response.ok, "{}", response.output);
        assert_eq!(response.status, "waiting");
        assert!(response.output.contains("queued task has no transcript"));
        assert!(!response.output.contains("queued task has not opened"));
        assert!(response.output.contains("admission waiting"));
        assert_eq!(response.output.matches("retry_at=123").count(), 1);
        assert!(test_web_queue_worker_spawned(&run.id));
    }

    #[test]
    fn task_chat_target_for_open_record_without_terminal_reports_queue_state_and_spawns_worker() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let run = queue_run_fixture("run-chat-eventfd-docs");
        let mut item = queue_item_fixture(&run.id, "iouring-rust-05-eventfd-docs");
        item.status = "waiting".into();
        item.message = "admission waiting: available memory is below requested reservation".into();
        state::replace_web_queue(&run, &[item]).unwrap();
        state::upsert_task_record(&task_record_fixture(
            "task/iouring-rust-05-eventfd-docs",
            "open",
        ))
        .unwrap();

        let err = ensure_task_chat_target("task/iouring-rust-05-eventfd-docs").unwrap_err();
        let output = format!("{err:#}");

        assert!(
            output.contains("queued task has no live task chat target yet"),
            "{output}"
        );
        assert!(!output.contains("task has no live chat target"));
        assert!(test_web_queue_worker_spawned(&run.id));
    }

    #[test]
    fn task_transcript_prefers_active_duplicate_slug_over_old_success_row() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let mut old_run = queue_run_fixture("aaa-old-eventfd-docs");
        old_run.status = "success".into();
        old_run.updated_at = 20;
        let mut old_item = queue_item_fixture(&old_run.id, "iouring-rust-05-eventfd-docs");
        old_item.id = "old-iouring-rust-05-eventfd-docs".into();
        old_item.status = "success".into();
        old_item.message = "old success row".into();
        old_item.updated_at = 20;
        state::replace_web_queue(&old_run, &[old_item]).unwrap();
        state::create_web_queue_tab("current", "Current").unwrap();
        state::activate_web_queue_tab("current").unwrap();
        let mut current_run = queue_run_fixture("zzz-current-eventfd-docs");
        current_run.updated_at = 1;
        let mut current_item =
            queue_item_fixture(&current_run.id, "iouring-rust-05-eventfd-docs");
        current_item.id = "current-iouring-rust-05-eventfd-docs".into();
        current_item.status = "waiting".into();
        current_item.message = "current admission waiting".into();
        current_item.updated_at = 1;
        state::replace_web_queue(&current_run, &[current_item]).unwrap();
        state::upsert_task_record(&task_record_fixture(
            "task/iouring-rust-05-eventfd-docs",
            "open",
        ))
        .unwrap();

        let response = task_transcript_response("task/iouring-rust-05-eventfd-docs");

        assert!(response.ok, "{}", response.output);
        assert_eq!(response.status, "waiting");
        assert!(response.output.contains("current admission waiting"));
        assert!(!response.output.contains("old success row"));
        assert!(test_web_queue_worker_spawned(&current_run.id));
        assert!(!test_web_queue_worker_spawned(&old_run.id));
    }

    fn queue_run_fixture(id: &str) -> state::QueueRunRow {
        state::QueueRunRow {
            id: id.to_string(),
            status: "running".into(),
            execution_mode: "graph".into(),
            execution_host: "local".into(),
            selected_agent_command: "c1".to_string(),
            remote_launcher: None,
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            selected_repo_root: None,
            selected_repo_name: None,
            track: id.to_string(),
            current_index: -1,
            stop_requested: false,
            message: "running".to_string(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn queue_item_fixture(run_id: &str, slug: &str) -> state::QueueItemRow {
        state::QueueItemRow {
            id: slug.to_string(),
            run_id: run_id.to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: format!("prompt {slug}"),
            slug: slug.to_string(),
            repo_root: None,
            repo_name: Some("qcold".to_string()),
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

    fn task_record_fixture(id: &str, status: &str) -> state::TaskRecordRow {
        state::TaskRecordRow {
            id: id.to_string(),
            source: "task-flow".to_string(),
            sequence: Some(42),
            title: id.strip_prefix("task/").unwrap_or(id).to_string(),
            description: "queued task record without live terminal".to_string(),
            status: status.to_string(),
            created_at: 1,
            updated_at: 2,
            repo_root: None,
            cwd: None,
            agent_id: Some("qa-missing-terminal".to_string()),
            metadata_json: None,
        }
    }
}
