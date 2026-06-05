#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod remote_adapter_tests {
    use super::{
        add_remote_adapter_metadata, canonical_remote_task_record, parse_task_record_json_lines,
        preserve_existing_remote_status, remote_adapter_label, remote_adapter_prefix_args,
        remote_task_open_env_words, remote_task_open_words, remote_task_record_export_words,
        RemoteAdapterArgs, RemoteTaskOpenEnvArgs,
    };
    use crate::state;
    use std::ffi::OsString;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn remote_adapter_defaults_to_cargo_xtask_contract() {
        let args = RemoteAdapterArgs {
            via: "remote-dev-env".to_string(),
            remote_adapter: "cargo".to_string(),
            adapter_args: Vec::new(),
            no_default_remote_adapter_arg: false,
        };

        assert_eq!(remote_adapter_label(&args), "cargo xtask");
        assert_eq!(remote_adapter_prefix_args(&args), os_args(&["xtask"]));
        assert_eq!(
            remote_task_open_words("remote-flow"),
            os_args(&["task", "open", "remote-flow"])
        );
        assert_eq!(
            remote_task_record_export_words(7),
            os_args(&["task", "export-records", "--limit", "7"])
        );
    }

    #[test]
    fn remote_adapter_can_use_direct_binary_without_default_arg() {
        let args = RemoteAdapterArgs {
            via: "remote-dev-env".to_string(),
            remote_adapter: "/opt/repo/xtask".to_string(),
            adapter_args: Vec::new(),
            no_default_remote_adapter_arg: true,
        };

        assert_eq!(remote_adapter_label(&args), "/opt/repo/xtask");
        assert_eq!(remote_adapter_prefix_args(&args), os_args(&[]));
    }

    #[test]
    fn remote_task_open_env_words_include_generic_names_and_repo_aliases() {
        let record = state::new_task_record(
            "task/remote-flow".to_string(),
            "task-flow".to_string(),
            "Remote Flow".to_string(),
            "Open remote flow".to_string(),
            "open".to_string(),
            Some("/local/repo".to_string()),
            None,
            None,
            Some(
                serde_json::json!({
                    "operator_prompt": "Run the remote flow"
                })
                .to_string(),
            ),
        );
        let env_args = RemoteTaskOpenEnvArgs {
            sequence_vars: vec!["VITASTOR_TASKFLOW_TASK_SEQUENCE".to_string()],
            prompt_names: vec!["VITASTOR_TASKFLOW_PROMPT".to_string()],
            description_keys: vec!["VITASTOR_TASKFLOW_DESCRIPTION".to_string()],
            thread_targets: Vec::new(),
            rollout_targets: Vec::new(),
        };

        let words = remote_task_open_env_words(&record, &env_args, 42);

        assert!(words.contains(&OsString::from("QCOLD_TASK_SEQUENCE=42")));
        assert!(words.contains(&OsString::from(
            "VITASTOR_TASKFLOW_TASK_SEQUENCE=42"
        )));
        assert!(words.contains(&OsString::from(
            "QCOLD_TASKFLOW_PROMPT=Run the remote flow"
        )));
        assert!(words.contains(&OsString::from(
            "VITASTOR_TASKFLOW_PROMPT=Run the remote flow"
        )));
        assert!(words.contains(&OsString::from(
            "QCOLD_TASKFLOW_DESCRIPTION=Open remote flow"
        )));
        assert!(words.contains(&OsString::from(
            "VITASTOR_TASKFLOW_DESCRIPTION=Open remote flow"
        )));
    }

    #[test]
    fn remote_task_record_export_parses_json_lines_only() {
        let record = state::TaskRecordRow {
            id: "task/remote-flow".to_string(),
            source: "task-flow".to_string(),
            sequence: Some(42),
            title: "remote-flow".to_string(),
            description: "Remote flow".to_string(),
            status: "open".to_string(),
            created_at: 10,
            updated_at: 11,
            repo_root: Some("/remote/repo".to_string()),
            cwd: Some("/remote/repo/task".to_string()),
            agent_id: None,
            metadata_json: Some("{\"remote\":true}".to_string()),
        };
        let raw = serde_json::to_string(&record).unwrap();
        let output = format!("noise\ntask-record-json\t{raw}\n");

        let parsed = parse_task_record_json_lines(&output).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "task/remote-flow");
        assert_eq!(parsed[0].sequence, Some(42));
    }

    #[test]
    fn remote_adapter_metadata_marks_launcher_and_adapter() {
        let mut record = state::new_task_record(
            "task/remote-flow".to_string(),
            "task-flow".to_string(),
            "Remote Flow".to_string(),
            "Open remote flow".to_string(),
            "open".to_string(),
            Some("/local/repo".to_string()),
            None,
            None,
            None,
        );
        let args = RemoteAdapterArgs {
            via: "remote-dev-env".to_string(),
            remote_adapter: "cargo".to_string(),
            adapter_args: Vec::new(),
            no_default_remote_adapter_arg: false,
        };

        add_remote_adapter_metadata(&mut record, &args, false);

        let metadata =
            serde_json::from_str::<serde_json::Value>(record.metadata_json.as_deref().unwrap())
                .unwrap();
        assert_eq!(metadata["remote_launcher"], "remote-dev-env");
        assert_eq!(metadata["remote_adapter"], "cargo xtask");
        assert_eq!(metadata["remote_adapter_legacy_qcold"], false);
    }

    #[test]
    fn remote_status_merge_preserves_existing_terminal_record() {
        let mut existing = state::new_task_record(
            "task/closed".to_string(),
            "task-flow".to_string(),
            "Closed".to_string(),
            "Closed task".to_string(),
            "closed:success".to_string(),
            Some("/local/repo".to_string()),
            None,
            None,
            None,
        );
        existing.updated_at = 200;
        let mut remote = existing.clone();
        remote.status = "open".to_string();
        remote.created_at = 100;
        remote.updated_at = 300;

        assert!(preserve_existing_remote_status(&existing, &remote));

        remote.status = "closed:failed".to_string();
        remote.updated_at = 100;
        assert!(preserve_existing_remote_status(&existing, &remote));

        remote.updated_at = 300;
        assert!(!preserve_existing_remote_status(&existing, &remote));
    }

    #[test]
    fn remote_status_merge_accepts_newer_reopened_task_record() {
        let mut existing = state::new_task_record(
            "task/reopened".to_string(),
            "task-flow".to_string(),
            "Reopened".to_string(),
            "Closed task".to_string(),
            "closed:blocked".to_string(),
            Some("/local/repo".to_string()),
            Some("/remote/repo".to_string()),
            None,
            None,
        );
        existing.created_at = 100;
        existing.updated_at = 200;
        let mut remote = existing.clone();
        remote.status = "open".to_string();
        remote.created_at = 300;
        remote.updated_at = 300;
        remote.cwd = Some("/remote/repo/reopened".to_string());

        assert!(!preserve_existing_remote_status(&existing, &remote));
        let merged = canonical_remote_task_record(&remote, Some(&existing), "/local/repo");
        assert_eq!(merged.status, "open");
        assert_eq!(merged.updated_at, 300);
        assert_eq!(merged.cwd.as_deref(), Some("/remote/repo/reopened"));
    }
}
