use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

const QCOLD_ENV_VARS: &[&str] = &[
    "QCOLD_ACTIVE_REPO",
    "QCOLD_REPO_ROOT",
    "QCOLD_TASKFLOW_PROMPT",
    "QCOLD_TASK_PROMPT_SNIPPET",
    "QCOLD_TASK_OPEN_BASE_BRANCH",
    "QCOLD_QUEUE_REMOTE_LAUNCHER",
    "QCOLD_QUEUE_REMOTE_AGENT_LAUNCHER_ENV",
    "QCOLD_REMOTE_TASK_RECORD_SYNC_TIMEOUT_SECONDS",
    "QCOLD_WEB_QUEUE_STATUS_SYNC_INTERVAL_SECONDS",
    "QCOLD_XTASK_MANIFEST",
    "QCOLD_STATE_DIR",
    "QCOLD_AGENT_OUTPUT_GUARD",
    "QCOLD_AGENT_OUTPUT_GUARD_COMMANDS",
    "QCOLD_OUTPUT_GUARD_ENABLED",
    "QCOLD_OUTPUT_GUARD_BIN",
    "QCOLD_OUTPUT_GUARD_COMMANDS",
    "QCOLD_GUARD_QCOLD",
    "QCOLD_GUARD_REAL_0_RG",
    "QCOLD_GUARD_REAL_1_GIT",
    "CODEX_HOME",
    "CODEX_ROLLOUT_PATH",
    "TELEGRAM_ENV_FILE",
    "JIRA_ENV_FILE",
    "TELEGRAM_API_BASE_URL",
    "TELEGRAM_NOTIFY_TIMEOUT",
    "QCOLD_TELEGRAM_OPERATOR_CHAT_ID",
    "TELEGRAM_CHAT_ID",
    "TELEGRAM_BOT_TOKEN",
    "JIRA_URL",
    "JIRA_PROJECT_KEY",
    "JIRA_ISSUE_TYPE",
    "JIRA_PARENT_KEY",
    "JIRA_DONE_TRANSITION",
    "JIRA_LABELS",
    "JIRA_SYNC",
    "JIRA_DEBUG_TO_TELEGRAM",
];

pub(crate) struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    cwd: PathBuf,
    path: Option<OsString>,
    vars: Vec<(&'static str, Option<String>)>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in &self.vars {
            match value {
                Some(value) => env::set_var(name, value),
                None => env::remove_var(name),
            }
        }
        match &self.path {
            Some(path) => env::set_var("PATH", path),
            None => env::remove_var("PATH"),
        }
        let _ = env::set_current_dir(&self.cwd);
    }
}

pub(crate) fn env_guard() -> EnvGuard {
    let lock = TEST_ENV_LOCK.lock().unwrap();
    let cwd = env::current_dir().unwrap();
    let path = env::var_os("PATH");
    let output_guard_bin = env::var_os("QCOLD_OUTPUT_GUARD_BIN").map(PathBuf::from);
    let vars = QCOLD_ENV_VARS
        .iter()
        .map(|name| (*name, env::var(name).ok()))
        .collect();
    if let Some(cleaned_path) = clean_test_path(path.as_ref(), output_guard_bin.as_deref()) {
        env::set_var("PATH", cleaned_path);
    }
    for name in QCOLD_ENV_VARS {
        env::remove_var(name);
    }
    EnvGuard {
        _lock: lock,
        cwd,
        path,
        vars,
    }
}

fn clean_test_path(path: Option<&OsString>, guard_bin: Option<&Path>) -> Option<OsString> {
    let path = path?;
    let paths = env::split_paths(path)
        .filter(|entry| {
            let is_current_guard = guard_bin.is_some_and(|guard_bin| entry == guard_bin);
            let is_qcold_guard = entry
                .to_string_lossy()
                .contains("/.local/state/qcold/guard-bin/");
            // Tests create fake remote launchers explicitly. Inheriting the operator's real
            // launcher makes remote-native unit tests observe external state.
            let has_external_remote_launcher = entry.join("remote-dev-env").is_file();
            !is_current_guard && !is_qcold_guard && !has_external_remote_launcher
        })
        .collect::<Vec<_>>();
    env::join_paths(paths).ok()
}
