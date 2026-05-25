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
            remote_launcher: None,
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
        assert!(instruction.contains("do not run qcold task open"));
        assert!(instruction.contains("task_env: .task/task.env"));
        assert!(instruction.contains("task_logs: .task/logs/"));
        assert!(instruction.contains("pause_or_blocked_only_for: business decision"));
        assert!(instruction.contains("output_guard:"));
        assert!(instruction.contains("automatically guard configured broad-output commands"));
        assert!(instruction.contains("qcold-guard status=blocked"));
        assert!(instruction.contains("operator_request_snippet: |\n  do focused work"));
        assert!(instruction.contains("operator_request: |\n  do focused work"));
        assert!(!instruction.contains("home base for /workspace/repo"));
    }

    #[test]
    fn queue_task_instruction_marks_remote_task_context() {
        let item = state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "do remote work".to_string(),
            slug: "task-remote-01".to_string(),
            repo_root: Some("/local/repo".to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            remote_launcher: Some("remote-dev-env".to_string()),
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };
        let task = QueueManagedTask {
            worktree: "/local/repo".into(),
            remote_launcher: Some("remote-dev-env".to_string()),
            remote_worktree: Some("/remote/WT/repo/task-remote-01".to_string()),
        };

        let instruction = queue_task_instruction_with_task(&item, &task);

        assert!(instruction.contains("remote_launcher: remote-dev-env"));
        assert!(instruction.contains("remote_task_worktree: /remote/WT/repo/task-remote-01"));
        assert!(instruction.contains("backend-opened remote managed task worktree"));
        assert!(instruction.contains("do not open a local task"));
    }

    #[test]
    fn queue_remote_agent_command_runs_through_launcher() {
        let item = state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "do remote work".to_string(),
            slug: "task-remote-01".to_string(),
            repo_root: Some("/local/repo".to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            remote_launcher: Some("remote-dev-env".to_string()),
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };
        let task = QueueManagedTask {
            worktree: "/local/repo".into(),
            remote_launcher: Some("remote-dev-env".to_string()),
            remote_worktree: Some("/remote/WT/repo/task-remote-01".to_string()),
        };

        let command = queue_agent_launch_command(&item, &task);

        assert!(command.starts_with("'remote-dev-env' sh -lc "));
        assert!(command.contains("/remote/WT/repo/task-remote-01"));
        assert!(command.contains("exec c1"));
        assert!(!command.starts_with("c1"));
    }

    #[test]
    fn queue_remote_policy_autoselects_default_launcher() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(
            repo.join("AGENTS.md"),
            "The default substantive execution environment is the approved remote dev\nenvironment.",
        )
        .unwrap();
        let launcher = bin.join("remote-dev-env");
        fs::write(&launcher, "#!/bin/sh\nexit 0\n").unwrap();
        make_executable(&launcher);

        assert_eq!(
            default_queue_remote_launcher_from(Some(repo.to_str().unwrap()), Some(bin.as_os_str())),
            Some("remote-dev-env".to_string())
        );
        assert_eq!(default_queue_remote_launcher_from(None, Some(bin.as_os_str())), None);
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
            remote_launcher: None,
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

    #[test]
    fn queue_task_record_agent_updates_after_executor_start() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        let worktree = temp.path().join("WT/repo/task-queue-agent");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        let item = state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "do focused work".to_string(),
            slug: "task-queue-agent".to_string(),
            repo_root: Some(repo.display().to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            remote_launcher: None,
            agent_id: None,
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };

        remember_queue_task_worktree(&item, &repo, &worktree).unwrap();
        remember_queue_task_agent(&item, "qa-task-queue-agent").unwrap();
        let record = state::get_task_record("task/task-queue-agent")
            .unwrap()
            .unwrap();

        assert_eq!(record.agent_id.as_deref(), Some("qa-task-queue-agent"));
    }

    #[test]
    fn queue_remote_task_record_metadata_preserves_launcher_and_worktree() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let item = state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "do remote work".to_string(),
            slug: "task-queue-remote".to_string(),
            repo_root: Some(repo.display().to_string()),
            repo_name: Some("repo".to_string()),
            agent_command: "c1".to_string(),
            remote_launcher: Some("remote-dev-env".to_string()),
            agent_id: Some("agent".to_string()),
            status: "pending".to_string(),
            message: String::new(),
            attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        };

        remember_queue_remote_task(&item, &repo, "remote-dev-env", "/remote/WT/repo/task").unwrap();
        let record = state::get_task_record("task/task-queue-remote")
            .unwrap()
            .unwrap();
        let metadata: serde_json::Value =
            serde_json::from_str(record.metadata_json.as_deref().unwrap()).unwrap();

        assert_eq!(record.repo_root.as_deref(), Some(repo.to_str().unwrap()));
        assert_eq!(record.cwd.as_deref(), Some("/remote/WT/repo/task"));
        assert_eq!(metadata["remote_launcher"].as_str(), Some("remote-dev-env"));
        assert_eq!(metadata["remote_cwd"].as_str(), Some("/remote/WT/repo/task"));
        assert_eq!(metadata["opened_by"].as_str(), Some("web-queue"));
    }

    fn make_executable(path: &std::path::Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }
}
