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
