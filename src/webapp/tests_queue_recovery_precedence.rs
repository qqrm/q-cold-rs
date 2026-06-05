#[cfg(test)]
mod queue_recovery_precedence_tests {
    #![allow(clippy::unwrap_used)]

    use crate::test_support;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn newer_blocked_repair_does_not_hide_open_related_repair() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let run = queue_run_fixture("graph-open-repair-record", "failed", 0);
        let mut item = queue_item_fixture(&run.id, "EBSR2-11", 0, "failed", Some("agent-11"));
        item.slug =
            "blockstore-ebs-v3f288-11-performance-parity-original-cpp-20260604-after-ebs00-p18332-20260605"
                .to_string();
        item.repo_root = Some(repo.clone());
        item.execution_host = "remote-native".to_string();
        item.remote_launcher = Some("remote-dev-env".to_string());
        item.message = "failed-closeout".to_string();
        item.started_at = 100;
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let mut failed = state::new_task_record(
            "task/blockstore-ebs-v3f288-11-performance-parity-original-cpp-20260604-after-ebs00-p18332-20260605"
                .to_string(),
            "task-flow".to_string(),
            "original failed 11".to_string(),
            "prompt failed 11".to_string(),
            "failed-closeout".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/original-11".to_string()),
            Some("agent-11".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        failed.updated_at = 110;
        state::upsert_task_record(&failed).unwrap();

        let mut open_repair = state::new_task_record(
            "task/blockstore-ebs-v3f288-11-current-fio-noprogress-repair-20260605".to_string(),
            "task-flow".to_string(),
            "11 repair".to_string(),
            "prompt 11 repair".to_string(),
            "open".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/open-repair-11".to_string()),
            Some("agent-open-repair".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        open_repair.updated_at = 120;
        state::upsert_task_record(&open_repair).unwrap();

        let mut blocked_duplicate = state::new_task_record(
            "task/blockstore-ebs-v3f288-11-performance-parity-repair2-20260605".to_string(),
            "task-flow".to_string(),
            "11 duplicate repair2".to_string(),
            "prompt 11 duplicate repair2".to_string(),
            "closed:blocked".to_string(),
            Some(repo),
            Some("/remote/repo/blocked-repair2-11".to_string()),
            Some("agent-blocked-repair2".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        blocked_duplicate.updated_at = 130;
        state::upsert_task_record(&blocked_duplicate).unwrap();

        assert_eq!(queue_task_status(&item).unwrap().as_deref(), Some("open"));
    }

    #[test]
    fn manual_open_repair_without_remote_launcher_supersedes_original_failed() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let run = queue_run_fixture("manual-open-repair-record", "failed", 0);
        let mut item = queue_item_fixture(&run.id, "EBSR2-11", 0, "failed", Some("agent-11"));
        item.slug =
            "blockstore-ebs-v3f288-11-performance-parity-original-cpp-20260604-after-ebs00-p18332-20260605"
                .to_string();
        item.repo_root = Some(repo.clone());
        item.execution_host = "remote-native".to_string();
        item.remote_launcher = Some("remote-dev-env".to_string());
        item.message = "closed:failed".to_string();
        item.started_at = 100;
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let mut failed = state::new_task_record(
            "task/blockstore-ebs-v3f288-11-performance-parity-original-cpp-20260604-after-ebs00-p18332-20260605"
                .to_string(),
            "task-flow".to_string(),
            "original failed 11".to_string(),
            "prompt failed 11".to_string(),
            "closed:failed".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/original-11".to_string()),
            Some("agent-11".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        failed.updated_at = 110;
        state::upsert_task_record(&failed).unwrap();

        let mut manual_repair = state::new_task_record(
            "task/blockstore-ebs-v3f288-11-current-fio-noprogress-repair-20260605".to_string(),
            "task-flow".to_string(),
            "11 manual repair".to_string(),
            "prompt 11 manual repair".to_string(),
            "open".to_string(),
            Some(repo),
            Some("/remote/repo/manual-repair-11".to_string()),
            Some("agent-manual-repair".to_string()),
            None,
        );
        manual_repair.updated_at = 120;
        state::upsert_task_record(&manual_repair).unwrap();

        assert_eq!(queue_task_status(&item).unwrap().as_deref(), Some("open"));
    }

    #[test]
    fn manual_success_repair_without_remote_launcher_supersedes_original_failed() {
        let _guard = test_support::env_guard();
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::create_dir(&repo).unwrap();
        std::env::set_var("QCOLD_STATE_DIR", &state_dir);
        let repo = repo.to_string_lossy().to_string();
        let run = queue_run_fixture("manual-success-repair-record", "failed", 0);
        let mut item = queue_item_fixture(&run.id, "EBSR2-11", 0, "failed", Some("agent-11"));
        item.slug =
            "blockstore-ebs-v3f288-11-performance-parity-original-cpp-20260604-after-ebs00-p18332-20260605"
                .to_string();
        item.repo_root = Some(repo.clone());
        item.execution_host = "remote-native".to_string();
        item.remote_launcher = Some("remote-dev-env".to_string());
        item.message = "closed:failed".to_string();
        item.started_at = 100;
        state::replace_web_queue(&run, &[item.clone()]).unwrap();

        let mut failed = state::new_task_record(
            "task/blockstore-ebs-v3f288-11-performance-parity-original-cpp-20260604-after-ebs00-p18332-20260605"
                .to_string(),
            "task-flow".to_string(),
            "original failed 11".to_string(),
            "prompt failed 11".to_string(),
            "closed:failed".to_string(),
            Some(repo.clone()),
            Some("/remote/repo/original-11".to_string()),
            Some("agent-11".to_string()),
            Some(r#"{"remote_launcher":"remote-dev-env"}"#.to_string()),
        );
        failed.updated_at = 110;
        state::upsert_task_record(&failed).unwrap();

        let mut manual_repair = state::new_task_record(
            "task/blockstore-ebs-v3f288-11-current-fio-noprogress-repair-20260605".to_string(),
            "task-flow".to_string(),
            "11 manual repair".to_string(),
            "prompt 11 manual repair".to_string(),
            "closed:success".to_string(),
            Some(repo),
            Some("/remote/repo/manual-repair-11".to_string()),
            Some("agent-manual-repair".to_string()),
            None,
        );
        manual_repair.updated_at = 120;
        state::upsert_task_record(&manual_repair).unwrap();

        assert_eq!(
            queue_task_status(&item).unwrap().as_deref(),
            Some("closed:success")
        );
    }

    fn queue_run_fixture(id: &str, status: &str, current_index: i64) -> state::QueueRunRow {
        state::QueueRunRow {
            id: id.to_string(),
            status: status.to_string(),
            execution_mode: "sequence".to_string(),
            execution_host: "local".to_string(),
            selected_agent_command: "c1".to_string(),
            remote_launcher: None,
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            selected_repo_root: None,
            selected_repo_name: None,
            track: "queue-run".to_string(),
            current_index,
            stop_requested: false,
            message: status.to_string(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn queue_item_fixture(
        run_id: &str,
        id: &str,
        position: i64,
        status: &str,
        agent_id: Option<&str>,
    ) -> state::QueueItemRow {
        state::QueueItemRow {
            id: id.to_string(),
            run_id: run_id.to_string(),
            position,
            depends_on: Vec::new(),
            prompt: format!("prompt {id}"),
            slug: format!("task-{id}"),
            repo_root: None,
            repo_name: None,
            execution_host: "local".to_string(),
            agent_command: "c1".to_string(),
            remote_launcher: None,
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            agent_id: agent_id.map(str::to_string),
            status: status.to_string(),
            message: String::new(),
            attempts: 0,
            recovery_attempts: 0,
            next_attempt_at: None,
            started_at: 0,
            updated_at: 0,
        }
    }
}
