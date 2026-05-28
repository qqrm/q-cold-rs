use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
#[allow(
    dead_code,
    reason = "xtask includes this module and uses the shared test env lock"
)]
pub(crate) static ROLLOUT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn current_codex_rollout_path(codex_thread_id: Option<&str>) -> Option<PathBuf> {
    current_codex_rollout_path_from_inputs(
        nonempty_env("CODEX_ROLLOUT_PATH").map(PathBuf::from),
        codex_thread_id,
        codex_session_roots_from_env(),
    )
}

fn current_codex_rollout_path_from_inputs(
    explicit_path: Option<PathBuf>,
    codex_thread_id: Option<&str>,
    roots: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    if let Some(path) = explicit_path {
        return Some(path);
    }
    current_codex_rollout_path_from_roots(codex_thread_id?, roots)
}

fn current_codex_rollout_path_from_roots(
    thread_id: &str,
    roots: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    let mut matches = Vec::new();
    for root in roots {
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
        assert_eq!(
            current_codex_rollout_path_from_inputs(
                Some(PathBuf::from("/tmp/current.jsonl")),
                Some("thread"),
                [PathBuf::from("/tmp/unused")],
            )
            .as_deref(),
            Some(Path::new("/tmp/current.jsonl"))
        );
    }

    #[test]
    fn finds_latest_rollout_by_thread_id_under_codex_home() {
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

        assert_eq!(
            current_codex_rollout_path_from_roots(thread_id, [codex_home.join("sessions")])
                .as_deref(),
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
}
