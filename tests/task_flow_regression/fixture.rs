mod fake_tools;

use assert_cmd::Command as AssertCommand;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use tempfile::{tempdir, TempDir};

pub(crate) use crate::task_flow_helpers::{
    git, git_output, load_task_env, seed_required_control_plane_files, write_exe, write_file,
    xtask_process_manifest, TaskEnv,
};

pub(crate) const BASE_BRANCH: &str = "developer";

fn test_binary_path(bin: &str) -> PathBuf {
    let binary_name = format!("{bin}{}", env::consts::EXE_SUFFIX);
    let env_key = format!("CARGO_BIN_EXE_{}", bin.replace('-', "_"));
    if let Ok(path) = env::var(&env_key) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return path;
        }
    }

    let mut candidates = Vec::new();
    if let Ok(container_root) = env::var("QCOLD_TASKFLOW_CONTAINER_ROOT") {
        candidates.push(
            PathBuf::from(container_root)
                .join("cargo-target/default/debug")
                .join(&binary_name),
        );
    }
    if let Ok(cargo_target_dir) = env::var("CARGO_TARGET_DIR") {
        candidates.push(
            PathBuf::from(&cargo_target_dir)
                .join("debug")
                .join(&binary_name),
        );
    }
    if let Ok(qcold) = env::var("CARGO_BIN_EXE_cargo_qcold") {
        candidates.push(PathBuf::from(qcold).with_file_name(&binary_name));
    }
    if let Ok(current_exe) = env::current_exe() {
        if let Some(debug_dir) = current_exe.parent().and_then(Path::parent) {
            candidates.push(debug_dir.join(&binary_name));
        }
    }

    if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
        return path;
    }

    panic!(
        "missing test binary {bin}; checked {env_key}, \
         QCOLD_TASKFLOW_CONTAINER_ROOT/cargo-target/default/debug, CARGO_TARGET_DIR/debug, \
         qcold sibling, and current_exe debug sibling"
    );
}

pub(crate) fn submodule_materialized(repo: &Path, path: &str) -> bool {
    let full = repo.join(path);
    if !full.is_dir() {
        return false;
    }
    full.join(".git").exists()
        || fs::read_dir(full)
            .ok()
            .and_then(|mut entries| entries.next())
            .is_some()
}

struct TelegramStub {
    base_url: String,
    mode: Arc<Mutex<String>>,
    requests: Arc<Mutex<Vec<String>>>,
    shutdown: mpsc::Sender<()>,
    handle: thread::JoinHandle<()>,
}

impl TelegramStub {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let mode = Arc::new(Mutex::new("ok".to_string()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let mode_clone = Arc::clone(&mode);
        let requests_clone = Arc::clone(&requests);
        let handle = thread::spawn(move || loop {
            if shutdown_rx.try_recv().is_ok() {
                break;
            }
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(std::time::Duration::from_millis(10));
                continue;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        content_length = value.trim().parse().unwrap();
                    }
                }
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            requests_clone.lock().unwrap().push(body.clone());
            let response = if *mode_clone.lock().unwrap() == "fail" {
                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 12\r\n\r\n{\"ok\":false}"
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}"
            };
            stream.write_all(response.as_bytes()).unwrap();
        });
        Self {
            base_url: format!("http://{addr}"),
            mode,
            requests,
            shutdown: shutdown_tx,
            handle,
        }
    }

    fn set_fail(&self) {
        *self.mode.lock().unwrap() = "fail".to_string();
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn last_request(&self) -> Option<String> {
        self.requests.lock().unwrap().last().cloned()
    }
}

impl Drop for TelegramStub {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
        let handle = std::mem::replace(&mut self.handle, thread::spawn(|| {}));
        let _ = handle.join();
    }
}

pub(crate) struct Fixture {
    pub(crate) temp: TempDir,
    pub(crate) remote: PathBuf,
    pub(crate) primary: PathBuf,
    pub(crate) fakebin: PathBuf,
    fakecontainerbin: PathBuf,
    docker_state: PathBuf,
    docker_images: PathBuf,
    devcontainer_log: PathBuf,
    glab_state: PathBuf,
    pub(crate) validation_log: PathBuf,
    notification_env_file: PathBuf,
    telegram: TelegramStub,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let temp = tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        let primary = temp.path().join("primary");
        let fakebin = temp.path().join("fakebin");
        let fakecontainerbin = temp.path().join("fakecontainerbin");
        let docker_state = temp.path().join("docker-state");
        let docker_images = temp.path().join("docker-images");
        let devcontainer_log = temp.path().join("devcontainer.log");
        let glab_state = temp.path().join("glab-state");
        let validation_log = temp.path().join("validation.log");
        let notification_env_file = temp.path().join("fixture-notify.env");

        fs::create_dir_all(&fakebin).unwrap();
        fs::create_dir_all(&fakecontainerbin).unwrap();
        fs::write(&docker_state, "").unwrap();
        fs::write(&docker_images, "").unwrap();
        fs::write(&devcontainer_log, "").unwrap();
        fs::write(&glab_state, "").unwrap();
        fs::write(&validation_log, "").unwrap();
        fs::write(&glab_state, "").unwrap();
        fs::write(&notification_env_file, "").unwrap();

        git_init_bare(&remote);
        git_clone(&remote, &primary);
        git(&primary, &["config", "user.name", "tester"]);
        git(&primary, &["config", "user.email", "tester@example.com"]);
        git(&primary, &["config", "taskflow.base-branch", BASE_BRANCH]);
        git(&primary, &["checkout", "-B", BASE_BRANCH]);
        seed_required_control_plane_files(&primary);
        write_file(&primary.join(".gitignore"), "bundles/\n");
        write_file(&primary.join("file.txt"), "base\n");
        write_file(
            &primary.join(".devcontainer/devcontainer.json"),
            r#"{
  "name": "task-flow-regression-fast",
  "build": {
    "dockerfile": "Dockerfile",
    "context": "..",
    "target": "devcontainer-fast"
  },
  "containerEnv": {
    "QCOLD_DEVCONTAINER_PROFILE": "fast"
  }
}
"#,
        );
        write_file(
            &primary.join(".devcontainer/full-qemu/devcontainer.json"),
            r#"{
  "name": "task-flow-regression-full-qemu",
  "build": {
    "dockerfile": "../Dockerfile",
    "context": "../..",
    "target": "devcontainer-full-qemu"
  },
  "containerEnv": {
    "QCOLD_DEVCONTAINER_PROFILE": "full-qemu"
  }
}
"#,
        );
        git(&primary, &["add", "."]);
        git(&primary, &["commit", "-m", "seed"]);
        git(&primary, &["push", "-u", "origin", BASE_BRANCH]);
        let _ = Command::new("git")
            .current_dir(&primary)
            .args(["remote", "set-head", "origin", BASE_BRANCH])
            .status();

        let sub1 = create_submodule_remote(temp.path(), "cpp-btree");
        let sub2 = create_submodule_remote(temp.path(), "json11");
        git(
            &primary,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                sub1.to_str().unwrap(),
                "cpp-btree",
            ],
        );
        git(
            &primary,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                sub2.to_str().unwrap(),
                "json11",
            ],
        );
        git(&primary, &["add", "."]);
        git(&primary, &["commit", "-m", "init"]);
        git(&primary, &["push", "origin", BASE_BRANCH]);
        git(&primary, &["submodule", "deinit", "-f", "--all"]);
        git(&primary, &["reset", "--hard", "HEAD"]);
        git(&primary, &["clean", "-ffd"]);

        fake_tools::write_fake_docker(&fakebin.join("docker"));
        fake_tools::write_fake_container_tools(&fakecontainerbin, &validation_log);
        fake_tools::write_fake_devcontainer(&fakebin.join("devcontainer"));
        fake_tools::write_fake_glab(&fakebin.join("glab"));

        Self {
            temp,
            remote,
            primary,
            fakebin,
            fakecontainerbin,
            docker_state,
            docker_images,
            devcontainer_log,
            glab_state,
            validation_log,
            notification_env_file,
            telegram: TelegramStub::start(),
        }
    }

    fn configure_taskflow_binary(
        &self,
        bin: &str,
        repo: &Path,
        args: &[&str],
        include_container_bin: bool,
        assume_container_runtime: bool,
    ) -> AssertCommand {
        let original_path = env::var("PATH").unwrap_or_default();
        let path = if include_container_bin {
            format!(
                "{}:{}:{}",
                self.fakecontainerbin.display(),
                self.fakebin.display(),
                original_path
            )
        } else {
            format!("{}:{}", self.fakebin.display(), original_path)
        };
        let mut cmd = AssertCommand::new(test_binary_path(bin));
        cmd.current_dir(repo)
            .args(args)
            .env_remove("QCOLD_TASKFLOW_CONTAINER_ROOT")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("QCOLD_TASKFLOW_CONTEXT")
            .env_remove("QCOLD_TASKFLOW_PRIMARY_REPO_PATH")
            .env_remove("QCOLD_TASKFLOW_TASK_ID")
            .env_remove("QCOLD_TASKFLOW_TASK_WORKTREE")
            .env_remove("QCOLD_TASKFLOW_TASK_BRANCH")
            .env_remove("QCOLD_TASKFLOW_DEVCONTAINER_ID")
            .env("QCOLD_REPO_ROOT", repo)
            .env("QCOLD_XTASK_MANIFEST", xtask_process_manifest())
            .env(
                "QCOLD_TASKFLOW_TEST_ASSUME_CONTAINER_RUNTIME",
                if assume_container_runtime { "1" } else { "0" },
            )
            .env("PATH", path)
            .env("FAKE_DOCKER_STATE", &self.docker_state)
            .env("FAKE_DOCKER_IMAGES", &self.docker_images)
            .env("FAKE_DEVCONTAINER_LOG", &self.devcontainer_log)
            .env("FAKE_DEVCONTAINER_CONTAINER_BIN", &self.fakecontainerbin)
            .env("FAKE_GLAB_STATE", &self.glab_state)
            .env("VALIDATION_RUN_LOG", &self.validation_log)
            .env("QCOLD_TASKFLOW_PROMPT", "Execution task fixture prompt")
            .env("QCOLD_TASKFLOW_AGENT_RUNNER", "codex")
            .env("QCOLD_TASKFLOW_AGENT_ID", "fixture-agent")
            .env("QCOLD_TASKFLOW_AGENT_MODEL", "gpt-5.4")
            .env("QCOLD_TASKFLOW_AGENT_STATUS", "healthy")
            .env("QCOLD_TASKFLOW_AGENT_REMAINING_CAPACITY", "42%")
            .env("QCOLD_TASKFLOW_USAGE_INPUT_TOKENS", "120")
            .env("QCOLD_TASKFLOW_USAGE_CACHED_INPUT_TOKENS", "20")
            .env("QCOLD_TASKFLOW_USAGE_OUTPUT_TOKENS", "181")
            .env("QCOLD_TASKFLOW_USAGE_TOTAL_TOKENS", "321")
            .env("QCOLD_TASKFLOW_USAGE_CREDITS", "1.25")
            .env("TELEGRAM_BOT_TOKEN", "test-token")
            .env("TELEGRAM_CHAT_ID", "test-chat")
            .env("TELEGRAM_API_BASE_URL", &self.telegram.base_url)
            .env("TELEGRAM_ENV_FILE", &self.notification_env_file)
            .env("JIRA_ENV_FILE", &self.notification_env_file);
        if assume_container_runtime {
            let task_env_path = repo.join(".task/task.env");
            if task_env_path.exists() {
                let task_env = load_task_env(repo);
                cmd.env("QCOLD_TASKFLOW_CONTEXT", TaskEnv::devcontainer_context())
                    .env(
                        "QCOLD_TASKFLOW_PRIMARY_REPO_PATH",
                        task_env.primary_repo_path.display().to_string(),
                    )
                    .env("QCOLD_TASKFLOW_TASK_ID", task_env.task_id.clone())
                    .env(
                        "QCOLD_TASKFLOW_TASK_WORKTREE",
                        task_env.task_worktree.display().to_string(),
                    )
                    .env("QCOLD_TASKFLOW_TASK_BRANCH", task_env.task_branch.clone())
                    .env(
                        "QCOLD_TASKFLOW_DEVCONTAINER_ID",
                        task_env.devcontainer_id.clone(),
                    )
                    .env(
                        "QCOLD_TASKFLOW_CONTAINER_ROOT",
                        task_env.container_runtime_root().display().to_string(),
                    )
                    .env(
                        "CARGO_TARGET_DIR",
                        task_env.container_cargo_target_dir().display().to_string(),
                    );
            }
        }
        cmd
    }

    fn configure_xtask(
        &self,
        repo: &Path,
        args: &[&str],
        include_container_bin: bool,
        assume_container_runtime: bool,
    ) -> AssertCommand {
        self.configure_taskflow_binary(
            "cargo-qcold",
            repo,
            args,
            include_container_bin,
            assume_container_runtime,
        )
    }

    pub(crate) fn run_xtask(&self, repo: &Path, args: &[&str]) -> AssertCommand {
        self.configure_xtask(repo, args, false, false)
    }

    pub(crate) fn run_qcold_in_container_runtime(
        &self,
        repo: &Path,
        args: &[&str],
    ) -> AssertCommand {
        self.configure_taskflow_binary("cargo-qcold", repo, args, true, true)
    }

    pub(crate) fn clear_devcontainer_log(&self) {
        fs::write(&self.devcontainer_log, "").unwrap();
    }

    pub(crate) fn devcontainer_log_text(&self) -> String {
        fs::read_to_string(&self.devcontainer_log).unwrap()
    }

    pub(crate) fn advance_base_branch(&self) -> String {
        let clone = self.temp.path().join(format!("advance-{}", rand_suffix()));
        git_clone(&self.remote, &clone);
        git(&clone, &["config", "user.name", "tester"]);
        git(&clone, &["config", "user.email", "tester@example.com"]);
        write_file(
            &clone.join("file.txt"),
            &format!("advanced-{}\n", rand_suffix()),
        );
        git(&clone, &["add", "file.txt"]);
        git(&clone, &["commit", "-m", "advance"]);
        git(&clone, &["push", "origin", BASE_BRANCH]);
        let head = git_output(&clone, &["rev-parse", "HEAD"]);
        fs::remove_dir_all(&clone).unwrap();
        head
    }

    pub(crate) fn telegram_request_count(&self) -> usize {
        self.telegram.request_count()
    }

    pub(crate) fn fail_telegram(&self) {
        self.telegram.set_fail();
    }

    pub(crate) fn last_telegram_request(&self) -> Option<String> {
        self.telegram.last_request()
    }
}

fn rand_suffix() -> String {
    format!(
        "{}",
        std::time::SystemTime::now().elapsed().unwrap().as_nanos()
    )
}

fn git_init_bare(path: &Path) {
    let status = Command::new("git")
        .args(["init", "--bare", "--initial-branch=developer"])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

pub(crate) fn git_clone(remote: &Path, dest: &Path) {
    let status = Command::new("git")
        .args(["clone", remote.to_str().unwrap(), dest.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
}

fn create_submodule_remote(root: &Path, name: &str) -> PathBuf {
    let remote = root.join(format!("{name}.git"));
    let clone = root.join(format!("{name}-work"));
    git_init_bare(&remote);
    git_clone(&remote, &clone);
    git(&clone, &["config", "user.name", "tester"]);
    git(&clone, &["config", "user.email", "tester@example.com"]);
    write_file(&clone.join("README.md"), name);
    git(&clone, &["add", "README.md"]);
    git(&clone, &["commit", "-m", "seed"]);
    git(&clone, &["push", "-u", "origin", BASE_BRANCH]);
    fs::remove_dir_all(&clone).unwrap();
    remote
}
