#[cfg(test)]
mod queue_prompt_tests {
    #![allow(clippy::unwrap_used)]

    use crate::{state, test_support};

    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn queue_task_instruction_starts_managed_task() {
        let item = state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "do focused work".to_string(),
            slug: "task-run-01".to_string(),
            repo_root: Some("/workspace/repo".to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };

        let instruction = queue_task_instruction(&item);
        assert!(instruction.contains("Q-COLD_TASK_PACKET"));
        assert!(instruction.contains("repo_root: /workspace/repo"));
        assert!(instruction.contains("task_slug: task-run-01"));
        assert!(instruction.contains("selected_command: c1"));
        assert!(instruction.contains("do not run cargo qcold task open"));
        assert!(instruction.contains("task_env: .task/task.env"));
        assert!(instruction.contains("task_logs: .task/logs/"));
        assert!(instruction.contains("pause_or_blocked_only_for: business decision"));
        assert!(instruction.contains("output_guard:"));
        assert!(instruction.contains("Q-COLD-started agents automatically guard broad"));
        assert!(instruction.contains("when automatic terminal guards do not apply"));
        assert!(instruction.contains("qcold guard -- <command>"));
        assert!(instruction.contains("operator_request_snippet: |\n  do focused work"));
        assert!(instruction.contains("operator_request: |\n  do focused work"));
        assert!(!instruction.contains("home base for /workspace/repo"));
    }

    #[test]
    fn queue_task_record_metadata_preserves_original_prompt_and_snippet() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        let worktree = temp.path().join("WT/repo/task-queue-01");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        let prompt = [
            "First line",
            "Second line",
            "Third line",
            "Fourth line",
            "Fifth line",
            "Sixth line should not be in snippet",
        ]
        .join("\n");
        let item = state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: prompt.clone(),
            slug: "task-queue-01".to_string(),
            repo_root: Some(repo.display().to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            agent_id: Some("agent".to_string()),
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };

        remember_queue_task_worktree(&item, &repo, &worktree).unwrap();
        let record = state::get_task_record("task/task-queue-01")
            .unwrap()
            .unwrap();
        let metadata: serde_json::Value =
            serde_json::from_str(record.metadata_json.as_deref().unwrap()).unwrap();

        assert_eq!(
            record.description,
            "First line\nSecond line\nThird line\nFourth line\nFifth line"
        );
        assert_eq!(metadata["operator_prompt"].as_str(), Some(prompt.as_str()));
        assert_eq!(
            metadata["operator_prompt_snippet"].as_str(),
            Some("First line\nSecond line\nThird line\nFourth line\nFifth line")
        );
        assert_eq!(metadata["prompt_source"].as_str(), Some("web-queue-card"));
    }
}
