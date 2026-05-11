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
}
