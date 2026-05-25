#[cfg(test)]
mod task_transcript_tests {
    #![allow(clippy::unwrap_used)]

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
}
