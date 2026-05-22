use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) static ROLLOUT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn current_codex_rollout_path(codex_thread_id: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = nonempty_env("CODEX_ROLLOUT_PATH").map(PathBuf::from) {
        return Some(path);
    }
    let thread_id = codex_thread_id?;
    let mut matches = Vec::new();
    for root in codex_session_roots_from_env() {
        matches.extend(find_rollout_paths_for_thread(&root, thread_id));
    }
    matches.sort();
    matches.pop()
}

fn codex_session_roots_from_env() -> Vec<PathBuf> {
    if let Some(home) = nonempty_env("CODEX_HOME") {
        return vec![PathBuf::from(home).join("sessions")];
    }
    let Some(home) = nonempty_env("HOME") else {
        return Vec::new();
    };
    let accounts = PathBuf::from(home).join(".codex-accounts");
    let Ok(entries) = fs::read_dir(accounts) else {
        return Vec::new();
    };
    let mut roots = entries
        .flatten()
        .map(|entry| entry.path().join("sessions"))
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    roots.sort();
    roots
}

fn find_rollout_paths_for_thread(root: &Path, thread_id: &str) -> Vec<PathBuf> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut stack = vec![root.to_path_buf()];
    let mut matches = Vec::new();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let is_jsonl = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"));
            let matches_thread = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(thread_id));
            if is_jsonl && matches_thread {
                matches.push(path);
            }
        }
    }
    matches
}

fn nonempty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "unit tests assert a narrow environment resolver contract"
    )]

    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn explicit_rollout_env_wins() {
        let _lock = ROLLOUT_ENV_LOCK.lock().unwrap();
        let _rollout = EnvVarGuard::capture("CODEX_ROLLOUT_PATH");
        std::env::set_var("CODEX_ROLLOUT_PATH", " /tmp/current.jsonl ");

        assert_eq!(
            current_codex_rollout_path(Some("thread")).as_deref(),
            Some(Path::new("/tmp/current.jsonl"))
        );
    }

    #[test]
    fn finds_latest_rollout_by_thread_id_under_codex_home() {
        let _lock = ROLLOUT_ENV_LOCK.lock().unwrap();
        let _rollout = EnvVarGuard::capture("CODEX_ROLLOUT_PATH");
        let _codex_home = EnvVarGuard::capture("CODEX_HOME");
        let _home = EnvVarGuard::capture("HOME");
        let root = unique_test_dir("qcold-shared-rollout-resolver");
        let codex_home = root.join("codex-home");
        let thread_id = "019e2a5a-96d5-72d0-9eaa-530232011047";
        let older = codex_home.join(format!(
            "sessions/2026/05/21/rollout-2026-05-21T23-00-00-{thread_id}.jsonl"
        ));
        let newer = codex_home.join(format!(
            "sessions/2026/05/22/rollout-2026-05-22T03-08-55-{thread_id}.jsonl"
        ));
        fs::create_dir_all(older.parent().unwrap()).unwrap();
        fs::create_dir_all(newer.parent().unwrap()).unwrap();
        fs::write(&older, "{}\n").unwrap();
        fs::write(&newer, "{}\n").unwrap();
        std::env::remove_var("CODEX_ROLLOUT_PATH");
        std::env::set_var("CODEX_HOME", &codex_home);

        assert_eq!(
            current_codex_rollout_path(Some(thread_id)).as_deref(),
            Some(newer.as_path())
        );

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let dir = std::env::temp_dir().join(format!("{name}-{}-{now}", std::process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
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
}
