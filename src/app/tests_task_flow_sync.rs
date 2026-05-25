#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod task_flow_sync_tests {
    use super::sync_task_flow_record_for_worktree;
    use crate::state;
    use std::fs;

    #[test]
    fn task_flow_sync_infers_live_web_queue_task_from_queue_row() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let slug = "queue-task-from-row";
        let worktree = temp.path().join("WT/qcold/002-queue-task-from-row");
        fs::create_dir_all(worktree.join(".task")).unwrap();
        fs::write(
            worktree.join(".task/task.env"),
            format!(
                "TASK_ID=task/{slug}\n\
                 TASK_NAME={slug}\n\
                 TASK_SEQUENCE=2\n\
                 TASK_WORKTREE={}\n\
                 TASK_DESCRIPTION=queue task\n\
                 PRIMARY_REPO_PATH={}\n\
                 STARTED_AT=1\n\
                 STATUS=open\n",
                worktree.display(),
                temp.path().join("qcold").display(),
            ),
        )
        .unwrap();
        let run = state::QueueRunRow {
            id: "queue-run".to_string(),
            status: "failed".to_string(),
            execution_mode: "graph".to_string(),
            selected_agent_command: "c1".to_string(),
            remote_launcher: None,
            selected_repo_root: Some(temp.path().join("qcold").display().to_string()),
            selected_repo_name: Some("qcold".to_string()),
            track: "queue-run".to_string(),
            current_index: 0,
            stop_requested: false,
            message: String::new(),
            created_at: 1,
            updated_at: 1,
        };
        let item = state::QueueItemRow {
            id: "item-1".to_string(),
            run_id: run.id.clone(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "queue task".to_string(),
            slug: slug.to_string(),
            repo_root: run.selected_repo_root.clone(),
            repo_name: run.selected_repo_name.clone(),
            agent_command: "c1".to_string(),
            remote_launcher: None,
            agent_id: None,
            status: "failed".to_string(),
            message: "task open interrupted".to_string(),
            attempts: 1,
            next_attempt_at: None,
            started_at: 1,
            updated_at: 1,
        };
        state::replace_web_queue(&run, &[item]).unwrap();
        let metadata = serde_json::json!({
            "session_path": "/tmp/creator.jsonl",
            "session_paths": ["/tmp/creator.jsonl"],
            "session_ids": ["creator"],
            "token_usage": {"total_tokens": 1},
            "token_efficiency": {"session_count": 1},
        });
        let record = state::new_task_record(
            format!("task/{slug}"),
            "task-flow".to_string(),
            "Queue Task".to_string(),
            "queue task".to_string(),
            "open".to_string(),
            Some(temp.path().join("qcold").display().to_string()),
            Some(worktree.display().to_string()),
            None,
            Some(metadata.to_string()),
        );
        state::upsert_task_record(&record).unwrap();

        assert!(sync_task_flow_record_for_worktree(&worktree, None).unwrap());
        let record = state::get_task_record(&format!("task/{slug}"))
            .unwrap()
            .unwrap();
        let metadata: serde_json::Value =
            serde_json::from_str(record.metadata_json.as_deref().unwrap()).unwrap();

        assert_eq!(metadata["opened_by"].as_str(), Some("web-queue"));
        assert!(metadata.get("session_path").is_none());
        assert!(metadata.get("session_paths").is_none());
        assert!(metadata.get("session_ids").is_none());
        assert!(metadata.get("token_usage").is_none());
        assert!(metadata.get("token_efficiency").is_none());
    }
}
