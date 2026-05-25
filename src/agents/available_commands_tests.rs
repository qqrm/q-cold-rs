#[cfg(test)]
mod available_commands_tests {
    #![allow(clippy::unwrap_used)]

    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn available_agent_commands_skip_unauthenticated_accounts() {
        let temp = tempdir().unwrap();
        let bin = temp.path().join("bin");
        let home = temp.path().join("home");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&home).unwrap();
        for command in ["c1", "c2", "codex4", "codex6"] {
            write_executable(&bin.join(command));
        }
        for account in ["1", "2", "4"] {
            write_auth_file(&home, account);
        }
        let path_env = env::join_paths([bin.as_path()]).unwrap();

        let commands = available_agent_commands_from(Some(path_env.as_os_str()), &home)
            .into_iter()
            .map(|agent| agent.command)
            .collect::<Vec<_>>();

        assert_eq!(commands, vec!["c1", "c2", "codex4"]);
    }

    fn write_auth_file(home: &Path, account: &str) {
        let path = agent_auth_file_in_home(account, home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "{}\n").unwrap();
    }

    fn write_executable(path: &Path) {
        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }
}
