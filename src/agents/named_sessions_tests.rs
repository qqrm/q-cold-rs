#[cfg(test)]
mod named_sessions_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use tempfile::tempdir;

    fn git_ok(cwd: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .current_dir(cwd)
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git command failed in {}: {:?}",
            cwd.display(),
            args
        );
    }

    fn seed_git_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        git_ok(path, &["init"]);
        git_ok(path, &["config", "user.name", "tester"]);
        git_ok(path, &["config", "user.email", "tester@example.com"]);
        fs::write(path.join("README.md"), "seed\n").unwrap();
        git_ok(path, &["add", "README.md"]);
        git_ok(path, &["commit", "-m", "seed"]);
    }

    fn insert_named_session(
        primary: &Path,
        id: &str,
        name: &str,
        started_at: u64,
        pid: u32,
    ) -> LaunchContext {
        let context = open_agent_worktree(id, "c1", started_at, primary).unwrap();
        state::insert_agent(&state::AgentRow {
            id: id.to_string(),
            track: "c1".to_string(),
            pid,
            started_at,
            command: vec![
                "zellij".to_string(),
                "--session".to_string(),
                format!("qcold-{id}"),
                "pane".to_string(),
                "1".to_string(),
                "/opt/qcold-test/bin/cc1".to_string(),
            ],
            cwd: Some(context.cwd.clone()),
            stdout_log_path: None,
            stderr_log_path: None,
        })
        .unwrap();
        state::save_terminal_metadata(&format!("zellij:qcold-{id}:1"), Some(name), None).unwrap();
        context
    }

    fn insert_codex_session_task(primary: &Path, context: &LaunchContext, agent_id: &str) {
        let metadata = serde_json::json!({
            "kind": "codex-session-import",
            "command": "/opt/qcold-test/bin/cc1",
            "codex_thread_id": "019df1ab-7579-7e41-ad71-701b63175455",
        });
        let record = state::new_task_record(
            format!("adhoc/{agent_id}"),
            "codex-session".to_string(),
            "Atomic".to_string(),
            "Atomic".to_string(),
            "closed:unknown".to_string(),
            Some(primary.display().to_string()),
            Some(context.cwd.display().to_string()),
            Some(agent_id.to_string()),
            Some(metadata.to_string()),
        );
        state::upsert_task_record(&record).unwrap();
    }

    fn write_terminal_exit_status(id: &str, status: i32) {
        let path = terminal_exit_status_path(id).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{status}\n")).unwrap();
    }

    fn agent_filter(agent: &str, name: Option<&str>) -> NamedSessionFilter {
        NamedSessionScopeArgs {
            agent: Some(agent.to_string()),
            track: None,
            account: None,
            repo_root: None,
        }
        .to_filter(name)
        .unwrap()
    }

    fn isolate_codex_home(temp: &Path) {
        env::set_var("CODEX_HOME", temp.join("codex"));
    }

    #[test]
    fn named_session_list_renders_resumable_rows() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        isolate_codex_home(temp.path());
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let context = insert_named_session(&primary, "old", "Atomic", 100, u32::MAX);
        insert_codex_session_task(&primary, &context, "old");
        write_terminal_exit_status("old", 130);

        let rows = named_session_rows(&agent_filter("cc1", None)).unwrap();
        let rendered = render_named_sessions(&rows);

        assert!(rendered.starts_with("named-sessions\tcount=1\n"));
        assert!(rendered.contains("named-session\tname=Atomic\ttrack=c1\taccount=1"));
        assert!(rendered.contains("\tresume=resumable\t"));
        assert!(rendered.contains("\tsession=019df1ab-7579-7e41-ad71-701b63175455"));
    }

    #[test]
    fn named_session_drop_removes_exited_binding_only() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        isolate_codex_home(temp.path());
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        let context = insert_named_session(&primary, "old", "Atomic", 100, u32::MAX);
        insert_codex_session_task(&primary, &context, "old");
        write_terminal_exit_status("old", 130);
        fs::create_dir_all(temp.path().join("state/logs")).unwrap();
        fs::write(log_path("old", "out").unwrap(), "output\n").unwrap();

        let summary = drop_named_sessions(&agent_filter("cc1", Some("Atomic")), false, false).unwrap();

        assert_eq!(summary.matched, 1);
        assert_eq!(summary.dropped, 1);
        assert_eq!(summary.deleted_agents, 1);
        assert_eq!(summary.deleted_task_records, 1);
        assert!(AgentState::load().unwrap().records.is_empty());
        assert!(state::load_terminal_metadata().unwrap().is_empty());
        let task_records = state::load_task_records(None, 1000).unwrap();
        assert!(task_records
            .iter()
            .all(|record| record.agent_id.as_deref() != Some("old")));
        assert!(!terminal_exit_status_path("old").unwrap().exists());
        assert!(!log_path("old", "out").unwrap().exists());
    }

    #[test]
    fn named_session_drop_all_requires_scope_or_all() {
        let args = NamedSessionsArgs {
            command: NamedSessionsCommand::DropAll(NamedSessionDropAllArgs {
                scope: NamedSessionScopeArgs::default(),
                all: false,
                dry_run: true,
                include_running: false,
            }),
        };

        let error = run_named_sessions(args).unwrap_err().to_string();

        assert!(error.contains("drop-all requires a scope"));
    }

    #[test]
    fn named_session_drop_all_skips_running_by_default() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        isolate_codex_home(temp.path());
        let primary = temp.path().join("repo");
        seed_git_repo(&primary);
        insert_named_session(&primary, "active", "Atomic", 100, std::process::id());

        let summary = drop_named_sessions(&agent_filter("cc1", None), false, false).unwrap();

        assert_eq!(summary.matched, 1);
        assert_eq!(summary.dropped, 0);
        assert_eq!(summary.skipped_running, 1);
        assert_eq!(AgentState::load().unwrap().records.len(), 1);
        assert_eq!(state::load_terminal_metadata().unwrap().len(), 1);
    }
}
