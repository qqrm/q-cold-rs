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

        write_task_env(&task).unwrap();

        let content = fs::read_to_string(worktree.join(".task/task.env")).unwrap();
        assert!(content.contains("TASK_DESCRIPTION=$'first line\\n"));
        assert_eq!(content.lines().count(), 16);

        let parsed = parse_task_env(&worktree.join(".task/task.env")).unwrap();

        assert_eq!(parsed.task_description, task.task_description);
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

    fn test_task_env() -> TaskEnv {
        TaskEnv {
            task_id: "task/pause".into(),
            task_name: "pause".into(),
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
        }
    }
}
