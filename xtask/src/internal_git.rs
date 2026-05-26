use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

const OUTPUT_GUARD_ENABLED_ENV: &str = "QCOLD_OUTPUT_GUARD_ENABLED";
const OUTPUT_GUARD_BIN_ENV: &str = "QCOLD_OUTPUT_GUARD_BIN";
const OUTPUT_GUARD_COMMAND_LIST_ENV: &str = "QCOLD_OUTPUT_GUARD_COMMANDS";
const OUTPUT_GUARD_QCOLD_ENV: &str = "QCOLD_GUARD_QCOLD";
const OUTPUT_GUARD_REAL_PREFIX: &str = "QCOLD_GUARD_REAL_";

pub(crate) fn command() -> Command {
    let vars = env::vars_os().collect::<Vec<_>>();
    command_from_env(&vars, false)
}

fn command_from_env(vars: &[(OsString, OsString)], seed_env: bool) -> Command {
    let program = real_git_from_guard_env(vars).unwrap_or_else(|| OsString::from("git"));
    let mut command = Command::new(program);
    if seed_env {
        command.envs(vars.iter().cloned());
    }
    scrub_inherited_output_guard(&mut command, vars);
    command
}

fn real_git_from_guard_env(vars: &[(OsString, OsString)]) -> Option<OsString> {
    vars.iter()
        .filter_map(|(key, value)| {
            let key = key.to_str()?;
            if value.to_string_lossy().trim().is_empty() {
                return None;
            }
            Some((guard_real_git_index(key)?, value.clone()))
        })
        .min_by_key(|(index, _)| *index)
        .map(|(_, value)| value)
}

fn guard_real_git_index(key: &str) -> Option<usize> {
    let rest = key.strip_prefix(OUTPUT_GUARD_REAL_PREFIX)?;
    if rest == "GIT" {
        return Some(usize::MAX);
    }
    let (index, command) = rest.split_once('_')?;
    (command == "GIT").then(|| index.parse().ok()).flatten()
}

fn scrub_inherited_output_guard(command: &mut Command, vars: &[(OsString, OsString)]) {
    command.env_remove(OUTPUT_GUARD_ENABLED_ENV);
    command.env_remove(OUTPUT_GUARD_BIN_ENV);
    command.env_remove(OUTPUT_GUARD_COMMAND_LIST_ENV);
    command.env_remove(OUTPUT_GUARD_QCOLD_ENV);
    for (key, _) in vars {
        if key
            .to_str()
            .is_some_and(|name| name.starts_with(OUTPUT_GUARD_REAL_PREFIX))
        {
            command.env_remove(key);
        }
    }
    for (key, _) in env::vars_os() {
        if key
            .to_str()
            .is_some_and(|name| name.starts_with(OUTPUT_GUARD_REAL_PREFIX))
        {
            command.env_remove(key);
        }
    }
    let Some(inherited_guard_bin) = env_value(vars, OUTPUT_GUARD_BIN_ENV).map(PathBuf::from) else {
        return;
    };
    let Some(path) = env_value(vars, "PATH") else {
        return;
    };
    let cleaned = path_without_output_guard_bin(path, &inherited_guard_bin);
    if cleaned != *path {
        command.env("PATH", cleaned);
    }
}

fn env_value<'a>(vars: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsString> {
    vars.iter()
        .find_map(|(key, value)| (key.to_str() == Some(name)).then_some(value))
}

fn path_without_output_guard_bin(path: &OsString, inherited_guard_bin: &Path) -> OsString {
    env::join_paths(
        env::split_paths(path)
            .filter(|dir| dir.as_path() != inherited_guard_bin)
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| OsString::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    #[cfg(unix)]
    fn command_uses_recorded_real_git_and_scrubs_guard_env() {
        let root = unique_test_dir("qcold-internal-git-real-env");
        let guard_bin = root.join("guard");
        let real_bin = root.join("real");
        fs::create_dir_all(&guard_bin).unwrap();
        fs::create_dir_all(&real_bin).unwrap();
        write_executable(
            &guard_bin.join("git"),
            "#!/bin/sh\nprintf 'guarded\\n'\nexit 44\n",
        );
        write_executable(
            &real_bin.join("git"),
            "#!/bin/sh\nprintf 'real\\nPATH=%s\\nENABLED=%s\\nREAL=%s\\n' \
             \"$PATH\" \"$QCOLD_OUTPUT_GUARD_ENABLED\" \"$QCOLD_GUARD_REAL_4_GIT\"\n",
        );

        let vars = test_env([
            (OUTPUT_GUARD_ENABLED_ENV, OsString::from("yes")),
            (OUTPUT_GUARD_BIN_ENV, guard_bin.clone().into_os_string()),
            (OUTPUT_GUARD_COMMAND_LIST_ENV, OsString::from("git")),
            (OUTPUT_GUARD_QCOLD_ENV, OsString::from("/tmp/qcold")),
            (
                "QCOLD_GUARD_REAL_4_GIT",
                real_bin.join("git").into_os_string(),
            ),
            (
                "PATH",
                env::join_paths([guard_bin.as_path(), real_bin.as_path()]).unwrap(),
            ),
        ]);

        let output = command_from_env(&vars, true).output().unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.starts_with("real\n"));
        assert!(!stdout.contains(&guard_bin.display().to_string()));
        assert!(stdout.contains("ENABLED=\n"));
        assert!(stdout.contains("REAL=\n"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn command_strips_guard_path_when_real_env_is_absent() {
        let root = unique_test_dir("qcold-internal-git-path-scrub");
        let guard_bin = root.join("guard");
        let real_bin = root.join("real");
        fs::create_dir_all(&guard_bin).unwrap();
        fs::create_dir_all(&real_bin).unwrap();
        write_executable(
            &guard_bin.join("git"),
            "#!/bin/sh\nprintf 'guarded\\n'\nexit 44\n",
        );
        write_executable(&real_bin.join("git"), "#!/bin/sh\nprintf 'real\\n'\n");

        let vars = test_env([
            (OUTPUT_GUARD_BIN_ENV, guard_bin.clone().into_os_string()),
            (
                "PATH",
                env::join_paths([guard_bin.as_path(), real_bin.as_path()]).unwrap(),
            ),
        ]);

        let output = command_from_env(&vars, true).output().unwrap();

        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "real\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, content: &str) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(path, content).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "{name}-{}-{}",
            std::process::id(),
            unix_timestamp_nanos()
        ));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn unix_timestamp_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    }

    fn test_env<const N: usize>(vars: [(&str, OsString); N]) -> Vec<(OsString, OsString)> {
        vars.into_iter()
            .map(|(key, value)| (OsString::from(key), value))
            .collect()
    }
}
