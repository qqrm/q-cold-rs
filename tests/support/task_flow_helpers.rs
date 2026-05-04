#![allow(
    dead_code,
    reason = "shared integration-test support is used by disjoint test binaries"
)]

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use tempfile::tempdir;

pub(crate) fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

pub(crate) fn git_output(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

pub(crate) fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

pub(crate) fn write_exe(path: &Path, contents: &str) {
    write_file(path, contents);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }
}

pub(crate) fn seed_required_control_plane_files(repo: &Path) {
    for path in required_control_plane_files() {
        write_file(&repo.join(path), "placeholder\n");
    }
}

pub(crate) fn xtask_process_manifest() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("repository/xtask/Cargo.toml");
    assert!(
        manifest.is_file(),
        "missing xtask process fixture adapter at {}",
        manifest.display()
    );
    manifest
}

pub(crate) fn parse_value(key: &str, text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::to_string))
}

pub(crate) fn bundle_listing(bundle: &Path) -> String {
    let output = Command::new("7z")
        .args(["l", "-slt", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "7z l failed");
    String::from_utf8(output.stdout).unwrap()
}

pub(crate) fn bundle_extract(bundle: &Path, needle: &str) -> String {
    let listing = bundle_listing(bundle);
    let bundle_name = bundle.file_name().unwrap().to_string_lossy().to_string();
    let entry = listing
        .lines()
        .filter_map(|line| line.strip_prefix("Path = "))
        .find(|line| *line != bundle_name && line.contains(needle))
        .unwrap_or_else(|| panic!("missing bundle entry {needle}"));
    let output = Command::new("7z")
        .args(["x", "-so", bundle.to_str().unwrap(), entry])
        .output()
        .unwrap();
    assert!(output.status.success(), "7z x -so failed for {entry}");
    String::from_utf8(output.stdout).unwrap()
}

pub(crate) fn bundle_extract_env(
    bundle: &Path,
    needle: &str,
) -> std::collections::BTreeMap<String, String> {
    let temp = tempdir().unwrap();
    let path = temp.path().join("bundle.env");
    fs::write(&path, bundle_extract(bundle, needle)).unwrap();
    parse_env_file(&path)
}

pub(crate) fn terminal_receipt_relative_path() -> &'static str {
    "metadata/terminal-receipt.env"
}

pub(crate) fn repository_receipt_relative_path() -> &'static str {
    "metadata/repository-receipt.env"
}

pub(crate) fn managed_root(primary_root: &Path) -> PathBuf {
    primary_root
        .parent()
        .unwrap()
        .join("WT")
        .join(primary_root.file_name().unwrap())
}

pub(crate) fn load_task_env(worktree: &Path) -> TaskEnv {
    TaskEnv::from_map(parse_env_file(&worktree.join(".task/task.env")))
}

pub(crate) fn save_task_env(worktree: &Path, task_env: &TaskEnv) {
    write_file(&worktree.join(".task/task.env"), &task_env.render());
}

pub(crate) fn parse_env_file(path: &Path) -> std::collections::BTreeMap<String, String> {
    parse_env_content(&fs::read_to_string(path).unwrap())
}

fn parse_env_content(content: &str) -> std::collections::BTreeMap<String, String> {
    let mut entries = std::collections::BTreeMap::new();
    let mut pending = String::new();
    for line in content.lines() {
        if pending.is_empty() && line.trim().is_empty() {
            continue;
        }
        if pending.is_empty() {
            pending.push_str(line);
        } else {
            pending.push('\n');
            pending.push_str(line);
        }
        if let Some((key, value)) = parse_env_entry(&pending) {
            entries.insert(key, value);
            pending.clear();
        }
    }
    assert!(pending.is_empty(), "invalid env content: {pending}");
    entries
}

fn parse_env_entry(line: &str) -> Option<(String, String)> {
    let (key, raw) = line.split_once('=')?;
    if !(raw.starts_with('\'') && raw.ends_with('\'')) {
        return Some((key.to_string(), raw.to_string()));
    }
    Some((key.to_string(), unquote_single(raw)))
}

fn unquote_single(raw: &str) -> String {
    let inner = &raw[1..raw.len() - 1];
    inner.replace("'\\''", "'")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn required_control_plane_files() -> Vec<&'static str> {
    vec![
        ".cargo/config.toml",
        ".githooks/pre-push",
        ".devcontainer/Dockerfile",
        ".devcontainer/devcontainer.json",
        ".devcontainer/full-qemu/devcontainer.json",
        ".devcontainer/post-create.sh",
        "AGENTS.md",
        "Cargo.lock",
        "Cargo.toml",
        "clippy.toml",
        "rust-toolchain.toml",
        "rustfmt.toml",
        "scripts/install-git-hooks.sh",
        "xtask/Cargo.toml",
        "xtask/src/ci.rs",
        "xtask/src/main.rs",
        "xtask/src/verify/gates.rs",
        "xtask/src/verify/preflight.rs",
        "tools/ffi-header-gen/Cargo.toml",
        "src/client/rust_client_ffi.h",
        "src/service/rust_service_ffi.h",
    ]
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) enum TaskStatus {
    #[default]
    Unset,
    Open,
    Other(String),
}

impl TaskStatus {
    fn parse(raw: &str) -> Self {
        match raw {
            "" => Self::Unset,
            "open" => Self::Open,
            other => Self::Other(other.to_string()),
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Unset => "",
            Self::Open => "open",
            Self::Other(value) => value.as_str(),
        }
    }
}

impl From<&str> for TaskStatus {
    fn from(value: &str) -> Self {
        Self::parse(value)
    }
}

impl From<String> for TaskStatus {
    fn from(value: String) -> Self {
        Self::parse(&value)
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct TaskEnv {
    pub(crate) task_id: String,
    pub(crate) task_name: String,
    pub(crate) task_branch: String,
    pub(crate) task_execution_anchor: String,
    pub(crate) task_description: String,
    pub(crate) task_worktree: PathBuf,
    pub(crate) task_profile: String,
    pub(crate) primary_repo_path: PathBuf,
    pub(crate) base_branch: String,
    pub(crate) base_head: String,
    pub(crate) task_head: String,
    pub(crate) started_at: String,
    pub(crate) status: TaskStatus,
    pub(crate) updated_at: String,
    pub(crate) last_bundle: String,
    pub(crate) devcontainer_id: String,
    pub(crate) devcontainer_name: String,
    pub(crate) delivery_mode: String,
    pub(crate) review_id: String,
    pub(crate) review_url: String,
    pub(crate) jira_issue_key: String,
    pub(crate) jira_issue_url: String,
    pub(crate) jira_creation_preview: String,
    pub(crate) jira_closeout_preview: String,
    pub(crate) delivered_head: String,
    pub(crate) merged_head: String,
}

impl TaskEnv {
    pub(crate) fn devcontainer_context() -> &'static str {
        "managed-task-devcontainer"
    }

    pub(crate) fn container_runtime_root(&self) -> PathBuf {
        PathBuf::from("/tmp/repository-taskflow").join(
            self.task_worktree
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("task")),
        )
    }

    pub(crate) fn container_cargo_target_dir(&self) -> PathBuf {
        self.container_runtime_root().join("cargo-target/default")
    }

    fn from_map(mut values: std::collections::BTreeMap<String, String>) -> Self {
        Self {
            task_id: take(&mut values, "TASK_ID"),
            task_name: take(&mut values, "TASK_NAME"),
            task_branch: take(&mut values, "TASK_BRANCH"),
            task_execution_anchor: take(&mut values, "TASK_EXECUTION_ANCHOR"),
            task_description: take(&mut values, "TASK_DESCRIPTION"),
            task_worktree: take(&mut values, "TASK_WORKTREE").into(),
            task_profile: take(&mut values, "TASK_PROFILE"),
            primary_repo_path: take(&mut values, "PRIMARY_REPO_PATH").into(),
            base_branch: take(&mut values, "BASE_BRANCH"),
            base_head: take(&mut values, "BASE_HEAD"),
            task_head: take(&mut values, "TASK_HEAD"),
            started_at: take(&mut values, "STARTED_AT"),
            status: TaskStatus::parse(&take(&mut values, "STATUS")),
            updated_at: take(&mut values, "UPDATED_AT"),
            last_bundle: take(&mut values, "LAST_BUNDLE"),
            devcontainer_id: take(&mut values, "DEVCONTAINER_ID"),
            devcontainer_name: take(&mut values, "DEVCONTAINER_NAME"),
            delivery_mode: take(&mut values, "DELIVERY_MODE"),
            review_id: take(&mut values, "REVIEW_ID"),
            review_url: take(&mut values, "REVIEW_URL"),
            jira_issue_key: take(&mut values, "JIRA_ISSUE_KEY"),
            jira_issue_url: take(&mut values, "JIRA_ISSUE_URL"),
            jira_creation_preview: take(&mut values, "JIRA_CREATION_PREVIEW"),
            jira_closeout_preview: take(&mut values, "JIRA_CLOSEOUT_PREVIEW"),
            delivered_head: take(&mut values, "DELIVERED_HEAD"),
            merged_head: take(&mut values, "MERGED_HEAD"),
        }
    }

    fn render(&self) -> String {
        let mut output = String::new();
        for (key, value) in [
            ("TASK_ID", self.task_id.as_str()),
            ("TASK_NAME", self.task_name.as_str()),
            ("TASK_BRANCH", self.task_branch.as_str()),
            ("TASK_EXECUTION_ANCHOR", self.task_execution_anchor.as_str()),
            ("TASK_DESCRIPTION", self.task_description.as_str()),
            ("TASK_WORKTREE", &self.task_worktree.display().to_string()),
            ("TASK_PROFILE", self.task_profile.as_str()),
            (
                "PRIMARY_REPO_PATH",
                &self.primary_repo_path.display().to_string(),
            ),
            ("BASE_BRANCH", self.base_branch.as_str()),
            ("BASE_HEAD", self.base_head.as_str()),
            ("TASK_HEAD", self.task_head.as_str()),
            ("STARTED_AT", self.started_at.as_str()),
            ("STATUS", self.status.as_str()),
            ("UPDATED_AT", self.updated_at.as_str()),
            ("LAST_BUNDLE", self.last_bundle.as_str()),
            ("DEVCONTAINER_ID", self.devcontainer_id.as_str()),
            ("DEVCONTAINER_NAME", self.devcontainer_name.as_str()),
            ("DELIVERY_MODE", self.delivery_mode.as_str()),
            ("REVIEW_ID", self.review_id.as_str()),
            ("REVIEW_URL", self.review_url.as_str()),
            ("JIRA_ISSUE_KEY", self.jira_issue_key.as_str()),
            ("JIRA_ISSUE_URL", self.jira_issue_url.as_str()),
            ("JIRA_CREATION_PREVIEW", self.jira_creation_preview.as_str()),
            ("JIRA_CLOSEOUT_PREVIEW", self.jira_closeout_preview.as_str()),
            ("DELIVERED_HEAD", self.delivered_head.as_str()),
            ("MERGED_HEAD", self.merged_head.as_str()),
        ] {
            let _ = writeln!(output, "{key}={}", shell_quote(value));
        }
        output
    }
}

fn take(values: &mut std::collections::BTreeMap<String, String>, key: &str) -> String {
    values.remove(key).unwrap_or_default()
}
