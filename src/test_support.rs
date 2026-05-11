use std::env;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

const QCOLD_ENV_VARS: &[&str] = &[
    "QCOLD_ACTIVE_REPO",
    "QCOLD_REPO_ROOT",
    "QCOLD_TASKFLOW_PROMPT",
    "QCOLD_TASK_PROMPT_SNIPPET",
    "QCOLD_XTASK_MANIFEST",
    "QCOLD_STATE_DIR",
];

pub(crate) struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    cwd: PathBuf,
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
        let _ = env::set_current_dir(&self.cwd);
    }
}

pub(crate) fn env_guard() -> EnvGuard {
    let lock = TEST_ENV_LOCK.lock().unwrap();
    let cwd = env::current_dir().unwrap();
    let vars = QCOLD_ENV_VARS
        .iter()
        .map(|name| (*name, env::var(name).ok()))
        .collect();
    for name in QCOLD_ENV_VARS {
        env::remove_var(name);
    }
    EnvGuard {
        _lock: lock,
        cwd,
        vars,
    }
}
