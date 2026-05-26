#[cfg(test)]
mod output_guard_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn terminal_env_prefix_prepends_output_guard_bin_to_path() {
        let output_guard = OutputGuardLaunch {
            bin_dir: PathBuf::from("/tmp/qcold guard/bin"),
            qcold_path: PathBuf::from("/opt/qcold/bin/qcold"),
            real_commands: vec![GuardedCommand {
                command: "rg".to_string(),
                env_name: "QCOLD_GUARD_REAL_0_RG".to_string(),
                real_path: PathBuf::from("/usr/bin/rg"),
            }],
        };

        let prefix =
            terminal_qcold_env_prefix_with_path(None, None, Some(&output_guard), Some("/usr/bin:/bin"));

        assert!(prefix.contains("export QCOLD_OUTPUT_GUARD_BIN='/tmp/qcold guard/bin';"));
        assert!(prefix.contains("export QCOLD_OUTPUT_GUARD_COMMANDS='rg';"));
        assert!(prefix.contains("export QCOLD_GUARD_QCOLD='/opt/qcold/bin/qcold';"));
        assert!(prefix.contains("export QCOLD_GUARD_REAL_0_RG='/usr/bin/rg';"));
        assert!(prefix.contains("export PATH='/tmp/qcold guard/bin:/usr/bin:/bin';"));
    }

    #[test]
    fn process_launch_env_prepends_output_guard_bin_to_path() {
        let _guard = crate::test_support::env_guard();
        env::set_var("PATH", "/usr/bin:/bin");
        let output_guard = OutputGuardLaunch {
            bin_dir: PathBuf::from("/tmp/qcold-guard/bin"),
            qcold_path: PathBuf::from("/opt/qcold/bin/qcold"),
            real_commands: vec![GuardedCommand {
                command: "rg".to_string(),
                env_name: "QCOLD_GUARD_REAL_0_RG".to_string(),
                real_path: PathBuf::from("/usr/bin/rg"),
            }],
        };
        let mut command = Command::new("rg");
        apply_qcold_launch_env(
            &mut command,
            Some(Path::new("/workspace/primary")),
            Some(Path::new("/workspace/WT/repo/agents/c1")),
            Some(&output_guard),
        );

        assert_eq!(
            command_env_value(&command, "QCOLD_REPO_ROOT").as_deref(),
            Some("/workspace/primary")
        );
        assert_eq!(
            command_env_value(&command, "QCOLD_AGENT_WORKTREE").as_deref(),
            Some("/workspace/WT/repo/agents/c1")
        );
        assert_eq!(
            command_env_value(&command, "QCOLD_OUTPUT_GUARD_BIN").as_deref(),
            Some("/tmp/qcold-guard/bin")
        );
        assert_eq!(
            command_env_value(&command, "QCOLD_OUTPUT_GUARD_COMMANDS").as_deref(),
            Some("rg")
        );
        assert_eq!(
            command_env_value(&command, "QCOLD_GUARD_REAL_0_RG").as_deref(),
            Some("/usr/bin/rg")
        );
        assert_eq!(
            command_env_value(&command, "PATH").as_deref(),
            Some("/tmp/qcold-guard/bin:/usr/bin:/bin")
        );
    }

    #[test]
    fn process_launch_env_removes_inherited_guard_bin_from_path() {
        let _guard = crate::test_support::env_guard();
        env::set_var("QCOLD_OUTPUT_GUARD_BIN", "/old/guard");
        env::set_var("PATH", "/old/guard:/usr/bin:/bin");
        let mut command = Command::new("rg");
        apply_qcold_launch_env(&mut command, None, None, None);
        assert_eq!(
            command_env_value(&command, "PATH").as_deref(),
            Some("/usr/bin:/bin")
        );
    }

    #[test]
    fn terminal_env_prefix_forwards_notification_env_without_token_literals() {
        let _guard = crate::test_support::env_guard();
        env::set_var("TELEGRAM_ENV_FILE", "/secure/taskflow-notify.env");
        env::set_var("TELEGRAM_API_BASE_URL", "http://127.0.0.1:1234");
        env::set_var("TELEGRAM_CHAT_ID", "test-chat");
        env::set_var("TELEGRAM_BOT_TOKEN", "secret-token");

        let prefix = terminal_qcold_env_prefix_with_path(None, None, None, Some("/usr/bin:/bin"));

        assert!(prefix.contains("export TELEGRAM_ENV_FILE='/secure/taskflow-notify.env';"));
        assert!(prefix.contains("export TELEGRAM_API_BASE_URL='http://127.0.0.1:1234';"));
        assert!(prefix.contains("export TELEGRAM_CHAT_ID='test-chat';"));
        assert!(!prefix.contains("TELEGRAM_BOT_TOKEN"));
        assert!(!prefix.contains("secret-token"));
    }

    #[test]
    fn terminal_env_prefix_discovers_repo_notification_env_file() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        let notify = temp.path().join(".env.taskflow-telegram.local");
        fs::write(&notify, "TELEGRAM_BOT_TOKEN=secret-token\n").unwrap();

        let prefix =
            terminal_qcold_env_prefix_with_path(Some(temp.path()), None, None, Some("/usr/bin"));
        let expected = notify.display().to_string();

        assert!(prefix.contains(&format!("export TELEGRAM_ENV_FILE={};", shell_quote(&expected))));
        assert!(prefix.contains(&format!("export JIRA_ENV_FILE={};", shell_quote(&expected))));
        assert!(!prefix.contains("secret-token"));
    }

    #[test]
    fn launch_env_discovers_primary_notification_file_for_task_worktree() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        let primary = temp.path().join("primary");
        let worktree = temp.path().join("task");
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(worktree.join(".task")).unwrap();
        let notify = primary.join(".env.taskflow-telegram.local");
        fs::write(&notify, "TELEGRAM_BOT_TOKEN=secret-token\n").unwrap();
        fs::write(
            worktree.join(".task/task.env"),
            format!("PRIMARY_REPO_PATH='{}'\n", primary.display()),
        )
        .unwrap();

        let mut command = Command::new("rg");
        apply_qcold_launch_env(&mut command, Some(&worktree), None, None);
        let expected = notify.display().to_string();

        assert_eq!(
            command_env_value(&command, "TELEGRAM_ENV_FILE").as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            command_env_value(&command, "JIRA_ENV_FILE").as_deref(),
            Some(expected.as_str())
        );
        assert!(command_env_value(&command, "TELEGRAM_BOT_TOKEN").is_none());

        let prefix =
            terminal_qcold_env_prefix_with_path(Some(&worktree), None, None, Some("/usr/bin"));
        assert!(prefix.contains(&format!("export TELEGRAM_ENV_FILE={};", shell_quote(&expected))));
        assert!(prefix.contains(&format!("export JIRA_ENV_FILE={};", shell_quote(&expected))));
        assert!(!prefix.contains("secret-token"));
    }

    #[test]
    fn output_guard_wrapper_uses_real_command_env_var() {
        let temp = tempdir().unwrap();
        let guarded = GuardedCommand {
            command: "rg".to_string(),
            env_name: "QCOLD_GUARD_REAL_RG".to_string(),
            real_path: PathBuf::from("/usr/bin/rg"),
        };

        write_output_guard_wrapper(temp.path(), &guarded).unwrap();

        let script = fs::read_to_string(temp.path().join("rg")).unwrap();
        assert!(script
            .contains("exec \"$QCOLD_GUARD_QCOLD\" guard -- \"$QCOLD_GUARD_REAL_RG\" \"$@\""));
        assert!(!script.contains(" guard -- rg "));
    }

    #[test]
    fn output_guard_skips_missing_commands() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        write_test_executable(&bin.join("rg"));
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));

        let launch = prepare_output_guard_launch_with_paths(
            "agent",
            123,
            vec!["rg".to_string(), "grep".to_string()],
            std::slice::from_ref(&bin),
            None,
        )
        .unwrap()
        .unwrap();

        assert!(launch.bin_dir.join("rg").is_file());
        assert!(!launch.bin_dir.join("grep").exists());
        assert_eq!(launch.real_commands.len(), 1);
        assert_eq!(launch.real_commands[0].command, "rg");
    }

    #[test]
    fn output_guard_disable_env_skips_wrapper_setup() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        env::set_var("QCOLD_AGENT_OUTPUT_GUARD", "0");

        assert!(prepare_output_guard_launch("agent", 123).unwrap().is_none());
        assert!(!temp.path().join("state/guard-bin").exists());
    }

    #[test]
    fn output_guard_resolution_skips_inherited_guard_bin() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        let inherited = temp.path().join("inherited");
        let real = temp.path().join("real");
        fs::create_dir_all(&inherited).unwrap();
        fs::create_dir_all(&real).unwrap();
        write_test_executable(&inherited.join("rg"));
        write_test_executable(&real.join("rg"));
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));

        let launch = prepare_output_guard_launch_with_paths(
            "agent",
            123,
            vec!["rg".to_string()],
            &[inherited.clone(), real.clone()],
            Some(inherited.as_path()),
        )
        .unwrap()
        .unwrap();

        assert_eq!(launch.real_commands[0].real_path, real.join("rg"));
    }

    #[test]
    fn terminal_env_prefix_removes_inherited_guard_bin_from_path() {
        let _guard = crate::test_support::env_guard();
        env::set_var("QCOLD_OUTPUT_GUARD_BIN", "/old/guard");

        let prefix =
            terminal_qcold_env_prefix_with_path(None, None, None, Some("/old/guard:/usr/bin:/bin"));

        assert!(prefix.contains("unset QCOLD_OUTPUT_GUARD_ENABLED QCOLD_OUTPUT_GUARD_BIN"));
        assert!(prefix.contains("QCOLD_OUTPUT_GUARD_COMMANDS QCOLD_GUARD_QCOLD;"));
        assert!(prefix.contains("export PATH='/usr/bin:/bin';"));
    }

    #[test]
    fn terminal_env_prefix_replaces_inherited_guard_bin_with_new_guard_bin() {
        let _guard = crate::test_support::env_guard();
        env::set_var("QCOLD_OUTPUT_GUARD_BIN", "/old/guard");
        let output_guard = OutputGuardLaunch {
            bin_dir: PathBuf::from("/new/guard"),
            qcold_path: PathBuf::from("/opt/qcold/bin/qcold"),
            real_commands: vec![GuardedCommand {
                command: "rg".to_string(),
                env_name: "QCOLD_GUARD_REAL_0_RG".to_string(),
                real_path: PathBuf::from("/usr/bin/rg"),
            }],
        };

        let prefix = terminal_qcold_env_prefix_with_path(
            None,
            None,
            Some(&output_guard),
            Some("/old/guard:/usr/bin:/bin"),
        );

        assert!(prefix.contains("export PATH='/new/guard:/usr/bin:/bin';"));
        assert!(!prefix.contains("PATH='/new/guard:/old/guard"));
    }

    #[test]
    fn output_guard_wraps_sed_only_when_configured() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        write_test_executable(&bin.join("sed"));
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));

        assert!(
            prepare_output_guard_launch_with_paths(
                "agent",
                123,
                vec![
                    "rg".to_string(),
                    "grep".to_string(),
                    "find".to_string(),
                    "cat".to_string(),
                    "git".to_string(),
                    "unzip".to_string(),
                    "zcat".to_string(),
                    "jq".to_string(),
                ],
                std::slice::from_ref(&bin),
                None,
            )
            .unwrap()
            .is_none()
        );

        let launch = prepare_output_guard_launch_with_paths(
            "agent",
            124,
            vec!["sed".to_string()],
            std::slice::from_ref(&bin),
            None,
        )
        .unwrap()
        .unwrap();
        assert!(launch.bin_dir.join("sed").is_file());
        assert_eq!(launch.real_commands[0].env_name, "QCOLD_GUARD_REAL_0_SED");
    }

    #[test]
    fn output_guard_custom_commands_get_distinct_env_names() {
        let _guard = crate::test_support::env_guard();
        let temp = tempdir().unwrap();
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        write_test_executable(&bin.join("foo-bar"));
        write_test_executable(&bin.join("foo_bar"));
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));

        let launch = prepare_output_guard_launch_with_paths(
            "agent",
            125,
            vec!["foo-bar".to_string(), "foo_bar".to_string()],
            std::slice::from_ref(&bin),
            None,
        )
        .unwrap()
        .unwrap();

        assert_eq!(launch.real_commands.len(), 2);
        assert_eq!(launch.real_commands[0].env_name, "QCOLD_GUARD_REAL_0_FOO_BAR");
        assert_eq!(launch.real_commands[1].env_name, "QCOLD_GUARD_REAL_1_FOO_BAR");
    }

    #[test]
    fn output_guard_default_commands_include_strict_agent_set() {
        let _guard = crate::test_support::env_guard();

        assert_eq!(
            output_guard_commands().unwrap(),
            vec!["rg", "grep", "find", "cat", "git", "unzip", "zcat", "jq"]
        );
    }

    #[test]
    fn output_guard_custom_commands_reject_invalid_names() {
        assert!(
            parse_output_guard_commands("rg,../cat").is_err(),
            "relative paths must not be accepted as wrapper command names"
        );
        assert!(
            parse_output_guard_commands("rg,git diff").is_err(),
            "command names with spaces must not be accepted"
        );
    }

    fn write_test_executable(path: &Path) {
        fs::write(path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    fn command_env_value(command: &Command, key: &str) -> Option<String> {
        command.get_envs().find_map(|(name, value)| {
            (name == key).then(|| {
                value
                    .and_then(std::ffi::OsStr::to_str)
                    .unwrap_or_default()
                    .to_string()
            })
        })
    }
}
