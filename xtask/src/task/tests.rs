#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sequence_anchor_is_zero_padded_operator_order() {
        assert_eq!(sequence_anchor(1).as_deref(), Some("001"));
        assert_eq!(sequence_anchor(42).as_deref(), Some("042"));
        assert_eq!(sequence_anchor(1001).as_deref(), Some("1001"));
        assert_eq!(sequence_anchor(0), None);
    }

    #[test]
    fn agent_return_worktree_reads_nonempty_env() {
        std::env::remove_var("QCOLD_AGENT_WORKTREE");
        assert_eq!(agent_return_worktree(), None);

        std::env::set_var("QCOLD_AGENT_WORKTREE", "  ");
        assert_eq!(agent_return_worktree(), None);

        std::env::set_var("QCOLD_AGENT_WORKTREE", "/workspace/WT/repo/agents/c1");
        assert_eq!(
            agent_return_worktree().as_deref(),
            Some(Path::new("/workspace/WT/repo/agents/c1"))
        );
        std::env::remove_var("QCOLD_AGENT_WORKTREE");
    }

    #[test]
    fn terminal_blocking_status_ignores_terminal_closeouts() {
        assert!(task_blocks_terminal(""));
        assert!(task_blocks_terminal("open"));
        assert!(task_blocks_terminal("paused"));
        assert!(task_blocks_terminal("failed-closeout"));
        assert!(!task_blocks_terminal("closed:success"));
        assert!(!task_blocks_terminal("closed:blocked"));
        assert!(!task_blocks_terminal("closed:failed"));
    }

    #[test]
    fn stale_paused_task_uses_updated_at_then_started_at() {
        let mut task = test_task_env();
        task.updated_at = "100".into();
        task.started_at = "1".into();
        assert!(task_is_stale(&task, 200, 50));
        assert!(!task_is_stale(&task, 120, 50));

        task.updated_at.clear();
        assert!(task_is_stale(&task, 200, 50));
    }

    #[test]
    fn task_env_round_trips_multiline_description() {
        let root = unique_test_dir("qcold-task-env-multiline");
        let worktree = root.join("task");
        let mut task = test_task_env();
        task.task_description = "first line\nsecond line with 'quote'\nthird\\line".into();
        task.task_worktree = worktree.clone();
        task.codex_thread_id = "019e2a5a-96d5-72d0-9eaa-530232011047".into();
        task.codex_rollout_path = "/tmp/rollout.jsonl".into();

        write_task_env(&task).unwrap();

        let content = fs::read_to_string(worktree.join(".task/task.env")).unwrap();
        assert!(content.contains("TASK_DESCRIPTION=$'first line\\n"));
        assert!(content.contains("CODEX_THREAD_ID=019e2a5a-96d5-72d0-9eaa-530232011047"));
        assert!(content.contains("CODEX_ROLLOUT_PATH=/tmp/rollout.jsonl"));
        assert_eq!(content.lines().count(), 19);

        let parsed = parse_task_env(&worktree.join(".task/task.env")).unwrap();

        assert_eq!(parsed.task_description, task.task_description);
        assert_eq!(parsed.codex_thread_id, task.codex_thread_id);
        assert_eq!(parsed.codex_rollout_path, task.codex_rollout_path);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_task_codex_env_finds_rollout_by_thread_id() {
        let _lock = crate::rollout::ROLLOUT_ENV_LOCK.lock().unwrap();
        let _rollout = EnvVarGuard::capture("CODEX_ROLLOUT_PATH");
        let _thread = EnvVarGuard::capture("CODEX_THREAD_ID");
        let _codex_home = EnvVarGuard::capture("CODEX_HOME");
        let root = unique_test_dir("qcold-rollout-resolver");
        let codex_home = root.join("codex-home");
        let thread_id = "019e2a5a-96d5-72d0-9eaa-530232011047";
        let rollout_path = codex_home.join(format!(
            "sessions/2026/05/22/rollout-2026-05-22T03-08-55-{thread_id}.jsonl"
        ));
        fs::create_dir_all(rollout_path.parent().unwrap()).unwrap();
        fs::write(&rollout_path, "{}\n").unwrap();
        std::env::remove_var("CODEX_ROLLOUT_PATH");
        std::env::remove_var("CODEX_THREAD_ID");
        std::env::set_var("CODEX_HOME", &codex_home);

        let mut task = test_task_env();
        task.codex_thread_id = thread_id.into();
        refresh_task_codex_env(&mut task);

        assert_eq!(task.codex_rollout_path, rollout_path.display().to_string());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn terminal_receipt_summarizes_worktree_conflicts() {
        let task_status =
            parse_worktree_status_summary("UU src/lib.rs\n?? notes.txt\n".to_string());
        let closeout_category = closeout_category("failed", &task_status);
        let receipt = TerminalReceipt {
            outcome: "failed",
            reason: Some("rebase conflict"),
            closeout_category,
            current_flow_problem: current_flow_problem("failed"),
            historical_flow_problem: historical_flow_problem(&task_status),
            closeout_failure_phase: None,
            closeout_failure_error: None,
            primary_clean: false,
            worktree_removed: true,
            branch_removed: false,
            primary_status: parse_worktree_status_summary(" M README.md\n".to_string()),
            task_status,
        };

        let rendered = render_terminal_receipt(&receipt);

        assert!(rendered.contains("CLOSEOUT_CATEGORY=task_worktree_conflicts"));
        assert!(rendered.contains("CURRENT_FLOW_PROBLEM=operator_failed"));
        assert!(rendered.contains("HISTORICAL_FLOW_PROBLEM=task_worktree_conflicts"));
        assert!(rendered.contains("CLOSEOUT_FAILURE_PHASE="));
        assert!(rendered.contains("PRIMARY_CHECKOUT_DIRTY_FILE_COUNT=1"));
        assert!(rendered.contains("TASK_WORKTREE_DIRTY_FILE_COUNT=2"));
        assert!(rendered.contains("TASK_WORKTREE_CONFLICT_FILE_COUNT=1"));
        assert!(rendered.contains("TASK_WORKTREE_CONFLICTS=src/lib.rs"));
        assert!(rendered.contains("TASK_WORKTREE_STATUS_SHORT=$'UU src/lib.rs\\n"));
    }

    #[test]
    fn failed_closeout_receipt_separates_current_and_historical_problems() {
        let task_status =
            parse_worktree_status_summary("UU src/lib.rs\n M README.md\n".to_string());
        let receipt = TerminalReceipt {
            outcome: "failed-closeout",
            reason: Some("closeout phase deliver-to-primary failed: task rebase failed"),
            closeout_category: closeout_category("failed-closeout", &task_status),
            current_flow_problem: current_flow_problem("failed-closeout"),
            historical_flow_problem: historical_flow_problem(&task_status),
            closeout_failure_phase: Some("deliver-to-primary"),
            closeout_failure_error: Some("task rebase failed"),
            primary_clean: false,
            worktree_removed: false,
            branch_removed: false,
            primary_status: parse_worktree_status_summary(" M README.md\n".to_string()),
            task_status,
        };

        let rendered = render_terminal_receipt(&receipt);

        assert!(rendered.contains("OUTCOME=failed-closeout"));
        assert!(rendered.contains("CLOSEOUT_CATEGORY=success_closeout_failed"));
        assert!(rendered.contains("CURRENT_FLOW_PROBLEM=success_closeout_failed"));
        assert!(rendered.contains("HISTORICAL_FLOW_PROBLEM=task_worktree_conflicts"));
        assert!(rendered.contains("CLOSEOUT_FAILURE_PHASE=deliver-to-primary"));
        assert!(rendered.contains("CLOSEOUT_FAILURE_ERROR='task rebase failed'"));
        assert!(rendered.contains("TASK_WORKTREE_REMOVED=no"));
        assert!(rendered.contains("LOCAL_TASK_BRANCH_REMOVED=no"));
    }

    #[test]
    fn failed_success_closeout_records_diagnostic_bundle_and_preserves_worktree() {
        if !seven_zip_available() {
            return;
        }
        let root = unique_test_dir("qcold-failed-closeout-diagnostic");
        let primary = root.join("primary");
        run_git_in(&root, ["init", path_arg(&primary)]);
        run_git_in(&primary, ["config", "user.name", "tester"]);
        run_git_in(&primary, ["config", "user.email", "tester@example.com"]);
        fs::write(primary.join("README.md"), "seed\n").unwrap();
        run_git_in(&primary, ["add", "README.md"]);
        run_git_in(&primary, ["commit", "-m", "seed"]);

        let worktree = root.join("task");
        run_git_in(
            &primary,
            [
                "worktree",
                "add",
                "-b",
                "task/closeout-fails",
                path_arg(&worktree),
                "HEAD",
            ],
        );
        fs::write(worktree.join("change.txt"), "dirty\n").unwrap();
        let mut task = TaskEnv {
            task_id: "task/closeout-fails".into(),
            task_name: "closeout-fails".into(),
            task_sequence: "2".into(),
            task_branch: "task/closeout-fails".into(),
            task_execution_anchor: "002".into(),
            task_description: "closeout failure".into(),
            task_worktree: worktree.clone(),
            task_profile: "default".into(),
            primary_repo_path: primary.clone(),
            base_branch: "main".into(),
            base_head: String::new(),
            task_head: String::new(),
            started_at: "1".into(),
            status: "open".into(),
            updated_at: "1".into(),
            devcontainer_name: "host-shell".into(),
            delivery_mode: "self-hosted-qcold".into(),
            codex_thread_id: String::new(),
            codex_rollout_path: String::new(),
        };

        record_success_closeout_failure(&mut task, "deliver-to-primary", "push failed").unwrap();

        assert_eq!(task.status, "failed-closeout");
        assert!(worktree.is_dir());
        let task_env = fs::read_to_string(worktree.join(".task/task.env")).unwrap();
        assert!(task_env.contains("STATUS=failed-closeout"));
        assert!(git_output(&primary, ["branch", "--list", "task/closeout-fails"])
            .unwrap()
            .contains("task/closeout-fails"));

        let bundle = fs::read_dir(primary.join("bundles"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().and_then(|value| value.to_str()) == Some("zip"))
            .unwrap();
        let extract = root.join("extract");
        fs::create_dir_all(&extract).unwrap();
        let status = std::process::Command::new("7z")
            .current_dir(&extract)
            .args(["x", path_arg(&bundle), "metadata/terminal-receipt.env"])
            .status()
            .unwrap();
        assert!(status.success());
        let receipt = fs::read_to_string(extract.join("metadata/terminal-receipt.env")).unwrap();
        assert!(receipt.contains("OUTCOME=failed-closeout"));
        assert!(receipt.contains("CURRENT_FLOW_PROBLEM=success_closeout_failed"));
        assert!(receipt.contains("HISTORICAL_FLOW_PROBLEM=task_worktree_dirty"));
        assert!(receipt.contains("CLOSEOUT_FAILURE_PHASE=deliver-to-primary"));
        assert!(receipt.contains("CLOSEOUT_FAILURE_ERROR='push failed'"));
        assert!(receipt.contains("TASK_WORKTREE_REMOVED=no"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_bundle_cleanup_removes_only_zip_files() {
        let root = unique_test_dir("qcold-bundle-cleanup");
        let bundles = root.join("bundles");
        fs::create_dir_all(&bundles).unwrap();
        fs::write(bundles.join("old.zip"), "zip").unwrap();
        fs::write(bundles.join("note.txt"), "note").unwrap();

        let cleanup = clear_stale_bundles(&root, 0).unwrap();

        assert_eq!(cleanup.removed, 1);
        assert!(!bundles.join("old.zip").exists());
        assert!(bundles.join("note.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preflight_profile_parses_stable_aliases() {
        let fast = PreflightProfile::parse(&[OsString::from("fast")]).unwrap();
        assert!(!fast.full);
        assert!(!fast.task_flow);

        let full = PreflightProfile::parse(&[OsString::from("full")]).unwrap();
        assert!(full.full);
        assert!(!full.task_flow);

        let task_flow =
            PreflightProfile::parse(&[OsString::from("--full"), OsString::from("task-flow")])
                .unwrap();
        assert!(task_flow.full);
        assert!(task_flow.task_flow);

        assert!(PreflightProfile::parse(&[OsString::from("unknown")]).is_err());
    }

    #[test]
    fn deliver_task_branch_pushes_base_and_refreshes_origin_tracking() {
        let root = unique_test_dir("qcold-self-closeout-push");
        let remote = root.join("remote.git");
        let primary = root.join("primary");

        run_git_in(&root, ["init", "--bare", path_arg(&remote)]);
        run_git_in(&root, ["clone", path_arg(&remote), path_arg(&primary)]);
        run_git_in(&primary, ["config", "user.name", "tester"]);
        run_git_in(&primary, ["config", "user.email", "tester@example.com"]);
        run_git_in(&primary, ["checkout", "-B", "main"]);
        fs::write(primary.join("README.md"), "seed\n").unwrap();
        run_git_in(&primary, ["add", "README.md"]);
        run_git_in(&primary, ["commit", "-m", "seed"]);
        run_git_in(&primary, ["push", "-u", "origin", "main"]);

        let worktree = root.join("task");
        run_git_in(
            &primary,
            [
                "worktree",
                "add",
                "-b",
                "task/push-proof",
                path_arg(&worktree),
                "HEAD",
            ],
        );
        fs::write(worktree.join("proof.txt"), "proof\n").unwrap();
        run_git_in(&worktree, ["add", "proof.txt"]);
        run_git_in(&worktree, ["commit", "-m", "add proof"]);

        let updater = root.join("updater");
        run_git_in(&root, ["clone", path_arg(&remote), path_arg(&updater)]);
        run_git_in(&updater, ["config", "user.name", "tester"]);
        run_git_in(&updater, ["config", "user.email", "tester@example.com"]);
        fs::write(updater.join("remote.txt"), "remote\n").unwrap();
        run_git_in(&updater, ["add", "remote.txt"]);
        run_git_in(&updater, ["commit", "-m", "advance remote"]);
        run_git_in(&updater, ["push", "origin", "main"]);

        let task = TaskEnv {
            task_id: "task/push-proof".into(),
            task_name: "push-proof".into(),
            task_sequence: "1".into(),
            task_branch: "task/push-proof".into(),
            task_execution_anchor: "001".into(),
            task_description: "push proof".into(),
            task_worktree: worktree,
            task_profile: "default".into(),
            primary_repo_path: primary.clone(),
            base_branch: "main".into(),
            base_head: git_output(&primary, ["rev-parse", "main"]).unwrap(),
            task_head: String::new(),
            started_at: "1".into(),
            status: "open".into(),
            updated_at: "1".into(),
            devcontainer_name: "host-shell".into(),
            delivery_mode: "self-hosted-qcold".into(),
            codex_thread_id: String::new(),
            codex_rollout_path: String::new(),
        };

        deliver_task_branch_to_primary(&task).unwrap();

        let local_main = git_output(&primary, ["rev-parse", "main"]).unwrap();
        let origin_main = git_output(&primary, ["rev-parse", "origin/main"]).unwrap();
        let remote_main = git_output(&remote, ["rev-parse", "refs/heads/main"]).unwrap();

        assert_eq!(local_main, origin_main);
        assert_eq!(local_main, remote_main);
        assert_eq!(
            fs::read_to_string(primary.join("proof.txt")).unwrap(),
            "proof\n"
        );
        assert_eq!(
            fs::read_to_string(primary.join("remote.txt")).unwrap(),
            "remote\n"
        );

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("{name}-{}-{}", std::process::id(), unix_now()));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run_git_in<const N: usize>(repo: &Path, args: [&str; N]) {
        run_git(repo, args).unwrap();
    }

    fn seven_zip_available() -> bool {
        std::process::Command::new("7z")
            .arg("--help")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn proof_run_index_records_task_identity_and_retains_latest_twenty() {
        let root = unique_test_dir("qcold-proof-run-index");
        let worktree = root.join("task");
        let summary_dir = worktree.join(".task/logs/compat/e2e-rust-blockstore");
        fs::create_dir_all(&summary_dir).unwrap();
        fs::write(
            summary_dir.join("summary.tsv"),
            "suite\tprofile\tbaseline_source\tbaseline_ref\tselected\tmatched\tregressions\t\
             timeouts\texecuted\treused_matched\n\
             rust-blockstore-e2e\tfull-qemu\tbaked\timage-sha256:abc\t59\t56\t2\t1\t59\t0\n",
        )
        .unwrap();
        fs::write(
            summary_dir.join("regressions.tsv"),
            "test\nheal-local-read\nheal-pg-size-2\n",
        )
        .unwrap();
        fs::write(summary_dir.join("timeouts.tsv"), "test\nnfs-unaligned-append\n").unwrap();

        let index = worktree.join(PROOF_RUN_INDEX);
        let mut existing = PROOF_RUN_INDEX_HEADER.join("\t");
        existing.push('\n');
        for sequence in 1..=20 {
            existing.push_str(&old_proof_run_row(sequence));
            existing.push('\n');
        }
        fs::create_dir_all(index.parent().unwrap()).unwrap();
        fs::write(&index, existing).unwrap();

        let mut task = test_task_env();
        task.task_id = "vitastor-123".into();
        task.task_name = "rust-blockstore-proof".into();
        task.task_sequence = "123".into();
        task.task_worktree = worktree.clone();
        task.task_profile = "full-qemu".into();
        task.task_head = "task-head-123".into();
        task.base_head = "base-head-122".into();

        update_proof_run_index(&task).unwrap();

        let content = fs::read_to_string(index).unwrap();
        let lines = content.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), PROOF_RUN_INDEX_LIMIT + 1);
        assert_eq!(lines[0], PROOF_RUN_INDEX_HEADER.join("\t"));
        assert!(!content.contains("bundles/"));
        assert!(!content.contains(".zip"));
        assert!(!content.contains("old-task-1\t"));
        assert!(content.contains(
            "123\tvitastor-123\trust-blockstore-proof\ttask-head-123\tbase-head-122\t\
             rust-blockstore-e2e\tfull-qemu\tbaked\timage-sha256:abc\t59\t56\t2\t1\t59\t0\t\
             fail\theal-local-read;heal-pg-size-2;nfs-unaligned-append"
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn proof_run_index_ignores_zero_selected_summaries() {
        let root = unique_test_dir("qcold-proof-run-index-zero");
        let worktree = root.join("task");
        let summary_dir = worktree.join(".task/logs/compat/e2e");
        fs::create_dir_all(&summary_dir).unwrap();
        fs::write(
            summary_dir.join("summary.tsv"),
            "selected\tmatched\tregressions\ttimeouts\n0\t0\t0\t0\n",
        )
        .unwrap();

        let mut task = test_task_env();
        task.task_worktree = worktree.clone();

        update_proof_run_index(&task).unwrap();

        assert!(!worktree.join(PROOF_RUN_INDEX).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn proof_run_index_accepts_compat_rows_without_selected() {
        let root = unique_test_dir("qcold-proof-run-index-compat");
        let worktree = root.join("task");
        let summary_dir = worktree.join(".task/logs/compat/blockstore");
        fs::create_dir_all(&summary_dir).unwrap();
        fs::write(
            summary_dir.join("summary.tsv"),
            "suite=blockstore-compat profile=matrix compat_rows=12 passed=12 regressions=0 \
             timeouts=0\n",
        )
        .unwrap();

        let mut task = test_task_env();
        task.task_worktree = worktree.clone();

        update_proof_run_index(&task).unwrap();

        let content = fs::read_to_string(worktree.join(PROOF_RUN_INDEX)).unwrap();
        assert!(content.contains("\tblockstore-compat\tmatrix\t\t\t12\t12\t0\t0\t\t\tpass\t"));
        fs::remove_dir_all(root).unwrap();
    }

    fn old_proof_run_row(sequence: u64) -> String {
        [
            sequence.to_string(),
            format!("old-task-{sequence}"),
            "old".to_string(),
            format!("head-{sequence}"),
            "base".to_string(),
            "old-suite".to_string(),
            "fast".to_string(),
            "none".to_string(),
            String::new(),
            "1".to_string(),
            "1".to_string(),
            "0".to_string(),
            "0".to_string(),
            "1".to_string(),
            "0".to_string(),
            "pass".to_string(),
            String::new(),
        ]
        .join("\t")
    }

    struct EnvVarGuard {
        name: &'static str,
        value: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn capture(name: &'static str) -> Self {
            Self {
                name,
                value: std::env::var_os(name),
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.value {
                std::env::set_var(self.name, value);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    fn test_task_env() -> TaskEnv {
        TaskEnv {
            task_id: "task/pause".into(),
            task_name: "pause".into(),
            task_sequence: "1".into(),
            task_branch: "task/pause".into(),
            task_execution_anchor: "001".into(),
            task_description: "pause".into(),
            task_worktree: PathBuf::from("/tmp/pause"),
            task_profile: "default".into(),
            primary_repo_path: PathBuf::from("/tmp/repo"),
            base_branch: "main".into(),
            base_head: "HEAD".into(),
            task_head: "HEAD".into(),
            started_at: "1".into(),
            status: "paused".into(),
            updated_at: "1".into(),
            devcontainer_name: "host-shell".into(),
            delivery_mode: "self-hosted-qcold".into(),
            codex_thread_id: String::new(),
            codex_rollout_path: String::new(),
        }
    }
}
