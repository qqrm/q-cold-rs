#[cfg(test)]
mod queue_taskflow_tests {
    use super::*;

    #[test]
    fn queue_task_open_output_reports_worktree() {
        let output = "task-opened\ttask-run-01\t/work/WT/repo/123-task-run-01\n\
                      TASK_WORKTREE=/work/WT/repo/123-task-run-01\n";

        assert_eq!(
            parse_task_worktree_output(output).unwrap(),
            PathBuf::from("/work/WT/repo/123-task-run-01")
        );
    }

    #[test]
    fn queue_task_open_prefers_runnable_current_executable() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("current-qcold");
        fs::write(&current, "").unwrap();
        make_executable(&current);

        assert_eq!(
            queue_qcold_executable_from(&current, None).unwrap(),
            current
        );
    }

    #[test]
    fn queue_task_open_falls_back_to_path_when_current_executable_was_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let installed = bin.join(format!("qcold{}", env::consts::EXE_SUFFIX));
        fs::write(&installed, "").unwrap();
        make_executable(&installed);

        let missing_current = temp.path().join("qcold (deleted)");
        let resolved = queue_qcold_executable_from(
            &missing_current,
            Some(std::ffi::OsStr::new(bin.to_str().unwrap())),
        )
        .unwrap();

        assert_eq!(resolved, installed);
    }

    #[test]
    fn queue_task_env_value_accepts_shell_quotes() {
        assert_eq!(shell_env_value("'task-run-01'"), "task-run-01");
        assert_eq!(shell_env_value("'task-'\\''run'"), "task-'run");
        assert_eq!(shell_env_value("task-run-01"), "task-run-01");
    }

    #[test]
    fn queue_remote_task_open_transport_failure_is_retryable() {
        let message = "failed to open remote managed task bs-meta3-perf-observability through \
                       remote-dev-env: exit status: 255\nssh: connect to host 10.253.244.101 \
                       port 22: Connection timed out";

        match queue_task_open_failure_outcome(message.to_string()) {
            QueueItemOutcome::Failed { retryable, .. } => assert!(retryable),
            _ => panic!("expected failed outcome"),
        }
    }

    #[test]
    fn queue_local_task_open_configuration_failure_is_not_retryable() {
        match queue_task_open_failure_outcome("queue item has no repository root".to_string()) {
            QueueItemOutcome::Failed { message, retryable } => {
                assert_eq!(message, "queue item has no repository root");
                assert!(!retryable);
            }
            _ => panic!("expected failed outcome"),
        }
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}
}
