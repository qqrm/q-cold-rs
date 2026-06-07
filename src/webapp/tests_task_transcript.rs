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
}
