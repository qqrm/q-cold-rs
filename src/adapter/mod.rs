use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

#[allow(
    dead_code,
    reason = "adapter contract is intentionally wider than current call sites"
)]
pub trait RepoAdapter {
    fn status_snapshot(&self) -> Result<String>;
}

pub trait TaskAdapter {
    fn inspect(&self, topic: Option<&str>) -> Result<u8>;
    fn open(
        &self,
        task_slug: &str,
        profile: Option<&str>,
        task_sequence: Option<u64>,
        task_prompt: Option<&str>,
    ) -> Result<u8>;
    fn enter(&self) -> Result<u8>;
    fn list(&self) -> Result<u8>;
    fn terminal_check(&self) -> Result<u8>;
    fn iteration_notify(&self, message: &str) -> Result<u8>;
    fn pause(&self, reason: &str) -> Result<u8>;
    fn closeout(&self, outcome: &str, message: Option<&str>, reason: Option<&str>) -> Result<u8>;
    fn finalize(&self, message: &str) -> Result<u8>;
    fn clean(&self, task_slug: &str) -> Result<u8>;
    fn clear(&self, task_slug: &str) -> Result<u8>;
    fn clear_all(&self) -> Result<u8>;
    fn orphan_list(&self) -> Result<u8>;
    fn orphan_clear_stale(&self, max_age_hours: u64) -> Result<u8>;
}

pub trait ProofAdapter {
    fn build(&self, args: &[OsString]) -> Result<u8>;
    fn install(&self, args: &[OsString]) -> Result<u8>;
    fn ci(&self, args: &[OsString]) -> Result<u8>;
    fn verify(&self, args: &[OsString]) -> Result<u8>;
    fn compat(&self, args: &[OsString]) -> Result<u8>;
    fn ffi(&self, args: &[OsString]) -> Result<u8>;
}

pub trait BundleAdapter {
    fn task_bundle(&self, task_id: Option<&str>) -> Result<u8>;
}

pub fn xtask_process_for(
    repo_root: &Path,
    xtask_manifest: Option<&Path>,
) -> Result<XtaskProcessAdapter> {
    XtaskProcessAdapter::discover_for(
        repo_root.to_path_buf(),
        xtask_manifest.map(Path::to_path_buf),
    )
}

pub struct XtaskProcessAdapter {
    repo_root: PathBuf,
    mode: XtaskMode,
}

enum XtaskMode {
    CargoSubcommand,
    Manifest(PathBuf),
}

impl XtaskProcessAdapter {
    pub fn discover_for(repo_root: PathBuf, xtask_manifest: Option<PathBuf>) -> Result<Self> {
        let mode = if let Some(path) = xtask_manifest {
            XtaskMode::Manifest(path)
        } else if let Ok(path) = env::var("QCOLD_XTASK_MANIFEST") {
            XtaskMode::Manifest(PathBuf::from(path))
        } else if repo_root.join("xtask/Cargo.toml").is_file() {
            XtaskMode::CargoSubcommand
        } else {
            let sibling = repo_root
                .parent()
                .map(|parent| parent.join("target-repo/xtask/Cargo.toml"));
            match sibling {
                Some(path) if path.is_file() => XtaskMode::Manifest(path),
                _ => bail!(
                    "xtask process adapter is unavailable; run from a target repository checkout, \
                     set QCOLD_XTASK_MANIFEST, or pass --xtask-manifest"
                ),
            }
        };
        Ok(Self { repo_root, mode })
    }

    fn run(&self, args: &[OsString]) -> Result<u8> {
        self.run_command(self.command(args)?, args)
    }

    fn run_command(&self, mut command: Command, args: &[OsString]) -> Result<u8> {
        let status = command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("failed to run xtask process adapter")?;
        let code = status.code().unwrap_or(1);
        if !status.success() {
            eprintln!(
                "Q-COLD adapter exited with code {code}: repo={} args={}",
                self.repo_root.display(),
                display_args(args)
            );
        }
        Ok(u8::try_from(code).unwrap_or(1))
    }

    #[allow(
        dead_code,
        reason = "RepoAdapter status capture is part of the adapter contract"
    )]
    fn capture(&self, args: &[OsString]) -> Result<String> {
        let output = self
            .command(args)?
            .output()
            .context("failed to run xtask process adapter")?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if !stdout.trim().is_empty() {
            return Ok(stdout);
        }
        if output.status.success() {
            return Ok(stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("xtask process adapter failed: {}", stderr.trim());
    }

    fn command(&self, args: &[OsString]) -> Result<Command> {
        let mut command = match &self.mode {
            XtaskMode::CargoSubcommand => {
                let mut command = Command::new("cargo");
                command.current_dir(&self.repo_root);
                command.arg("xtask");
                command
            }
            XtaskMode::Manifest(manifest) => {
                let mut command = Command::new(manifest_binary(manifest)?);
                command.current_dir(&self.repo_root);
                command
            }
        };
        command.args(args);
        Ok(command)
    }

    fn run_words(&self, args: &[&str]) -> Result<u8> {
        self.run(&os_args(args))
    }
}

impl RepoAdapter for XtaskProcessAdapter {
    fn status_snapshot(&self) -> Result<String> {
        self.capture(&os_args(&["task", "terminal-check"]))
    }
}

impl TaskAdapter for XtaskProcessAdapter {
    fn inspect(&self, topic: Option<&str>) -> Result<u8> {
        let mut args = os_args(&["task", "inspect"]);
        push_optional(&mut args, topic);
        self.run(&args)
    }

    fn open(
        &self,
        task_slug: &str,
        profile: Option<&str>,
        task_sequence: Option<u64>,
        task_prompt: Option<&str>,
    ) -> Result<u8> {
        let mut args = os_args(&["task", "open", task_slug]);
        push_optional(&mut args, profile);
        let mut command = self.command(&args)?;
        apply_task_open_env(&mut command, task_sequence, task_prompt);
        self.run_command(command, &args)
    }

    fn enter(&self) -> Result<u8> {
        self.run_words(&["task", "enter"])
    }

    fn list(&self) -> Result<u8> {
        self.run_words(&["task", "list"])
    }

    fn terminal_check(&self) -> Result<u8> {
        self.run_words(&["task", "terminal-check"])
    }

    fn iteration_notify(&self, message: &str) -> Result<u8> {
        self.run(&os_args(&[
            "task",
            "iteration-notify",
            "--message",
            message,
        ]))
    }

    fn pause(&self, reason: &str) -> Result<u8> {
        self.run(&os_args(&["task", "pause", "--reason", reason]))
    }

    fn closeout(&self, outcome: &str, message: Option<&str>, reason: Option<&str>) -> Result<u8> {
        let mut args = os_args(&["task", "closeout", "--outcome", outcome]);
        if let Some(message) = message {
            args.push("--message".into());
            args.push(message.into());
        }
        if let Some(reason) = reason {
            args.push("--reason".into());
            args.push(reason.into());
        }
        self.run(&args)
    }

    fn finalize(&self, message: &str) -> Result<u8> {
        self.run(&os_args(&["task", "finalize", "--message", message]))
    }

    fn clean(&self, task_slug: &str) -> Result<u8> {
        self.run(&os_args(&["task", "clean", task_slug]))
    }

    fn clear(&self, task_slug: &str) -> Result<u8> {
        self.run(&os_args(&["task", "clear", task_slug]))
    }

    fn clear_all(&self) -> Result<u8> {
        self.run_words(&["task", "clear-all"])
    }

    fn orphan_list(&self) -> Result<u8> {
        self.run_words(&["task", "orphan-list"])
    }

    fn orphan_clear_stale(&self, max_age_hours: u64) -> Result<u8> {
        self.run(&os_args(&[
            "task",
            "orphan-clear-stale",
            "--max-age-hours",
            &max_age_hours.to_string(),
        ]))
    }
}

impl ProofAdapter for XtaskProcessAdapter {
    fn build(&self, args: &[OsString]) -> Result<u8> {
        self.run_prefixed("build", args)
    }

    fn install(&self, args: &[OsString]) -> Result<u8> {
        self.run_prefixed("install", args)
    }

    fn ci(&self, args: &[OsString]) -> Result<u8> {
        self.run_prefixed("ci", args)
    }

    fn verify(&self, args: &[OsString]) -> Result<u8> {
        self.run_prefixed("verify", args)
    }

    fn compat(&self, args: &[OsString]) -> Result<u8> {
        self.run_prefixed("compat", args)
    }

    fn ffi(&self, args: &[OsString]) -> Result<u8> {
        self.run_prefixed("ffi", args)
    }
}

impl BundleAdapter for XtaskProcessAdapter {
    fn task_bundle(&self, task_id: Option<&str>) -> Result<u8> {
        let mut args = os_args(&["task", "bundle"]);
        push_optional(&mut args, task_id);
        self.run(&args)
    }
}

impl XtaskProcessAdapter {
    fn run_prefixed(&self, prefix: &str, args: &[OsString]) -> Result<u8> {
        let mut full_args = Vec::with_capacity(args.len() + 1);
        full_args.push(prefix.into());
        full_args.extend(args.iter().cloned());
        self.run(&full_args)
    }
}

fn os_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn push_optional(args: &mut Vec<OsString>, value: Option<&str>) {
    if let Some(value) = value {
        args.push(value.into());
    }
}

fn display_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn apply_task_open_env(
    command: &mut Command,
    task_sequence: Option<u64>,
    task_prompt: Option<&str>,
) {
    let codex_thread_id = nonempty_env("CODEX_THREAD_ID");
    let codex_rollout_path = current_codex_rollout_path(codex_thread_id.as_deref());
    apply_task_open_env_values(
        command,
        task_sequence,
        task_prompt,
        codex_thread_id.as_deref(),
        codex_rollout_path.as_deref(),
    );
}

fn apply_task_open_env_values(
    command: &mut Command,
    task_sequence: Option<u64>,
    task_prompt: Option<&str>,
    codex_thread_id: Option<&str>,
    codex_rollout_path: Option<&Path>,
) {
    if let Some(sequence) = task_sequence {
        command.env("QCOLD_TASK_SEQUENCE", sequence.to_string());
    }
    if let Some(prompt) = task_prompt.map(str::trim).filter(|value| !value.is_empty()) {
        command.env("QCOLD_TASKFLOW_PROMPT", prompt);
        command.env("QCOLD_TASK_PROMPT_SNIPPET", crate::prompt::prompt_snippet(prompt));
    }
    if let Some(thread_id) = codex_thread_id.map(str::trim).filter(|value| !value.is_empty()) {
        command.env("CODEX_THREAD_ID", thread_id);
    }
    if let Some(path) = codex_rollout_path {
        command.env("CODEX_ROLLOUT_PATH", path);
    }
}

fn current_codex_rollout_path(codex_thread_id: Option<&str>) -> Option<PathBuf> {
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
    let Some(home) = env::var("HOME").ok() else {
        return Vec::new();
    };
    let accounts = PathBuf::from(home).join(".codex-accounts");
    let Ok(entries) = fs::read_dir(accounts) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path().join("sessions"))
        .filter(|path| path.is_dir())
        .collect()
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
            } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl")
                && path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.contains(thread_id))
            {
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
        reason = "unit tests assert a narrow command environment contract"
    )]

    use super::*;

    #[test]
    fn task_open_passes_qcold_sequence_and_prompt_to_adapter_process() {
        let mut command = Command::new("cargo");
        apply_task_open_env_values(
            &mut command,
            Some(42),
            Some("first line\nsecond line"),
            Some("019e2a5a-96d5-72d0-9eaa-530232011047"),
            Some(Path::new("/tmp/rollout.jsonl")),
        );

        let sequence = command
            .get_envs()
            .find_map(|(key, value)| {
                (key == "QCOLD_TASK_SEQUENCE").then(|| {
                    value
                        .and_then(std::ffi::OsStr::to_str)
                        .unwrap_or_default()
                        .to_string()
                })
            })
            .unwrap();
        let prompt = command
            .get_envs()
            .find_map(|(key, value)| {
                (key == "QCOLD_TASKFLOW_PROMPT").then(|| {
                    value
                        .and_then(std::ffi::OsStr::to_str)
                        .unwrap_or_default()
                        .to_string()
                })
            })
            .unwrap();
        let snippet = command
            .get_envs()
            .find_map(|(key, value)| {
                (key == "QCOLD_TASK_PROMPT_SNIPPET").then(|| {
                    value
                        .and_then(std::ffi::OsStr::to_str)
                        .unwrap_or_default()
                        .to_string()
                })
            })
            .unwrap();
        let thread_id = command
            .get_envs()
            .find_map(|(key, value)| {
                (key == "CODEX_THREAD_ID").then(|| {
                    value
                        .and_then(std::ffi::OsStr::to_str)
                        .unwrap_or_default()
                        .to_string()
                })
            })
            .unwrap();
        let rollout_path = command
            .get_envs()
            .find_map(|(key, value)| {
                (key == "CODEX_ROLLOUT_PATH").then(|| {
                    value
                        .and_then(std::ffi::OsStr::to_str)
                        .unwrap_or_default()
                        .to_string()
                })
            })
            .unwrap();

        assert_eq!(sequence, "42");
        assert_eq!(prompt, "first line\nsecond line");
        assert_eq!(snippet, "first line\nsecond line");
        assert_eq!(thread_id, "019e2a5a-96d5-72d0-9eaa-530232011047");
        assert_eq!(rollout_path, "/tmp/rollout.jsonl");
    }
}

fn manifest_binary(manifest: &Path) -> Result<PathBuf> {
    let manifest = manifest
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", manifest.display()))?;
    let xtask_dir = manifest
        .parent()
        .context("xtask process manifest has no parent directory")?;
    let workspace_root = xtask_dir
        .parent()
        .context("xtask process directory has no workspace root")?;
    let binary = workspace_root
        .join("target")
        .join("debug")
        .join(format!("xtask{}", env::consts::EXE_SUFFIX));
    let status = Command::new("cargo")
        .current_dir(workspace_root)
        .args(["build", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .context("failed to build xtask process adapter")?;
    if !status.success() {
        bail!("failed to build xtask process adapter");
    }

    if !binary.is_file() {
        bail!(
            "xtask process adapter binary was not built at {}",
            binary.display()
        );
    }
    Ok(binary)
}
