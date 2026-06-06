#[cfg(test)]
mod queue_prompt_tests {
    #![allow(clippy::unwrap_used)]

    use std::fs;

    use crate::{state, test_support};

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn queue_task_instruction_delegates_task_environment_selection() {
        let item = queue_prompt_item("task-run-01", None);

        let instruction = queue_task_instruction(&item);

        assert!(instruction.contains("Q-COLD_TASK_PACKET"));
        assert!(instruction.contains("repo_root: /workspace/repo"));
        assert!(instruction.contains("task_slug: task-run-01"));
        assert!(instruction.contains("selected_command: c1"));
        assert!(instruction.contains("executor-owned task environment selection"));
        assert!(instruction.contains("Q-COLD has not opened task_slug"));
        assert!(instruction.contains("has not selected a profile or container"));
        assert!(instruction.contains("start or resume the repository-managed task-flow"));
        assert!(instruction.contains("choose the required repo-approved environment"));
        assert!(instruction.contains("task_env: <actual-task-worktree>/.task/task.env"));
        assert!(instruction.contains("task_logs: <actual-task-worktree>/.task/logs/"));
        assert!(instruction.contains("make task/<task_slug> visible to local Q-COLD"));
        assert!(instruction.contains("pause_or_blocked_only_for: business decision"));
        assert!(instruction.contains("output_guard:"));
        assert!(instruction.contains("automatically guard configured broad-output commands"));
        assert!(instruction.contains("qcold-guard status=blocked"));
        assert!(instruction.contains("operator_request_snippet: |\n  do focused work"));
        assert!(instruction.contains("operator_request: |\n  do focused work"));
        assert!(!instruction.contains("do not run qcold task open"));
        assert!(!instruction.contains("backend-opened"));
    }

    #[test]
    fn queue_task_instruction_marks_available_remote_launcher_as_hint() {
        let item = queue_prompt_item("task-remote-01", Some("remote-dev-env"));
        let task = QueueLaunchWorkspace {
            worktree: "/local/repo".into(),
            remote_launcher: Some("remote-dev-env".to_string()),
            remote_worktree: None,
            existing_task: false,
        };

        let instruction = queue_task_instruction_with_task(&item, &task);

        assert!(instruction.contains("available_remote_launcher: remote-dev-env"));
        assert!(instruction.contains("executor-owned task environment selection"));
        assert!(instruction.contains("available_remote_launcher is a convenience launcher"));
        assert!(instruction.contains("not a selected profile"));
        assert!(instruction.contains("choose the required repo-approved environment"));
        assert!(!instruction.contains("remote_task_worktree:"));
        assert!(!instruction.contains("Q-COLD already opened the remote task"));
        assert!(!instruction.contains("do not open a local task"));
    }

    #[test]
    fn queue_task_instruction_allows_existing_remote_task_resume() {
        let item = queue_prompt_item("task-remote-01", Some("remote-dev-env"));
        let task = QueueLaunchWorkspace {
            worktree: "/local/repo".into(),
            remote_launcher: Some("remote-dev-env".to_string()),
            remote_worktree: Some("/remote/WT/repo/task-remote-01".to_string()),
            existing_task: true,
        };

        let instruction = queue_task_instruction_with_task(&item, &task);

        assert!(instruction.contains("available_remote_launcher: remote-dev-env"));
        assert!(instruction.contains("remote_task_worktree: /remote/WT/repo/task-remote-01"));
        assert!(instruction.contains("launch_context: existing remote managed task record"));
        assert!(instruction.contains("re-enter it if it still matches the goal"));
        assert!(instruction.contains("Q-COLD did not choose a new profile"));
        assert!(instruction.contains("task_env: remote_task_worktree/.task/task.env"));
        assert!(instruction.contains("task_logs: remote_task_worktree/.task/logs/"));
        assert!(instruction.contains("sync local Q-COLD if needed"));
    }

    #[test]
    fn queue_local_agent_command_uses_codex_exec_stdin_packet() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        let item = queue_prompt_item("task-remote-01", Some("remote-dev-env"));
        let task = QueueLaunchWorkspace {
            worktree: "/local/repo".into(),
            remote_launcher: Some("remote-dev-env".to_string()),
            remote_worktree: None,
            existing_task: false,
        };

        let prompt_file = write_queue_task_packet_file(&item, &task).unwrap();
        let command = queue_agent_launch_command(&item, &task, &prompt_file);
        let packet = fs::read_to_string(&prompt_file).unwrap();

        assert_eq!(
            command,
            format!(
                "c1 exec --dangerously-bypass-approvals-and-sandbox -C /local/repo - < {}",
                queue_shell_quote(&prompt_file.display().to_string())
            )
        );
        assert!(packet.contains("Q-COLD_TASK_PACKET"));
        assert!(packet.contains("task_slug: task-remote-01"));
        assert!(prompt_file.starts_with(temp.path().join("state/queue-task-packets")));
    }

    #[test]
    fn remote_native_packet_marks_remote_executor_contract() {
        let mut item = queue_prompt_item("task-remote-native-01", Some("remote-dev-env"));
        item.execution_host = "remote-native".into();

        let instruction = queue_remote_native_task_instruction(&item);

        assert!(instruction.contains("execution_host: remote-native"));
        assert!(instruction.contains("repository remote-agent contract"));
        assert!(instruction.contains("do not reopen task_slug"));
        assert!(instruction.contains("local launcher Q-COLD used to reach this remote session"));
        assert!(instruction.contains("Codex executor chat is running on the remote host"));
        assert!(!instruction.contains("keep this Codex executor chat local"));
    }

    #[test]
    fn queue_task_record_agent_updates_after_executor_start() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        std::env::set_var("QCOLD_STATE_DIR", temp.path());
        let item = queue_prompt_item("task-queue-agent", None);
        let record = state::new_task_record(
            "task/task-queue-agent".to_string(),
            "task-flow".to_string(),
            "task-queue-agent".to_string(),
            "existing task".to_string(),
            "open".to_string(),
            Some("/workspace/repo".to_string()),
            Some("/workspace/WT/repo/task-queue-agent".to_string()),
            None,
            None,
        );
        state::upsert_task_record(&record).unwrap();

        remember_queue_task_agent(&item, "qa-task-queue-agent").unwrap();
        let record = state::get_task_record("task/task-queue-agent")
            .unwrap()
            .unwrap();

        assert_eq!(record.agent_id.as_deref(), Some("qa-task-queue-agent"));
    }

    fn queue_prompt_item(slug: &str, remote_launcher: Option<&str>) -> state::QueueItemRow {
        state::QueueItemRow {
            id: "item".to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "do focused work".to_string(),
            slug: slug.to_string(),
            repo_root: Some("/workspace/repo".to_string()),
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
}
