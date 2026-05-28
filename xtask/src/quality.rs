use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const MAX_TEXT_FILE_LINES: usize = 1_000;
const MAX_TEXT_LINE_WIDTH: usize = 120;

const LARGE_FILE_EXCEPTIONS: &[LargeFileException] = &[
    LargeFileException {
        path: "Cargo.lock",
        reason: "Cargo owns lockfile shape",
    },
    LargeFileException {
        path: "src/queue.rs",
        reason: "Queue CLI/package parser split is pending; keep contract changes in one surface",
    },
    LargeFileException {
        path: "src/state.rs",
        reason:
            "State facade split is pending; queue row API still lives with shared state entrypoints",
    },
    LargeFileException {
        path: "src/state/db.rs",
        reason: "Schema bootstrap and migrations stay ordered inline until db module split",
    },
    LargeFileException {
        path: "src/webapp/tests.rs",
        reason: "Shared dashboard regression fixture split is pending",
    },
];

struct LargeFileException {
    path: &'static str,
    reason: &'static str,
}

pub(crate) fn run(repo: &Path) -> Result<()> {
    let tracked = tracked_files(repo)?;
    reject_python_skill_scripts(repo, &tracked)?;
    reject_large_text_files(repo, &tracked)?;
    reject_long_text_lines(repo, &tracked)
}

fn tracked_files(repo: &Path) -> Result<Vec<PathBuf>> {
    let output = crate::internal_git::command()
        .args(["ls-files", "-z"])
        .current_dir(repo)
        .output()
        .context("failed to list tracked files")?;
    if !output.status.success() {
        bail!("git ls-files failed with status {}", output.status);
    }
    let mut paths = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn reject_large_text_files(repo: &Path, paths: &[PathBuf]) -> Result<()> {
    let mut violations = Vec::new();
    for relative in paths {
        let Some(text) = tracked_text(repo, relative)? else {
            continue;
        };
        let line_count = text.lines().count();
        if line_count > MAX_TEXT_FILE_LINES && !large_file_allowed(relative) {
            violations.push(format!(
                "{}:{line_count}>{MAX_TEXT_FILE_LINES}",
                relative.display()
            ));
        }
    }
    if violations.is_empty() {
        return Ok(());
    }
    bail!(
        "tracked text files exceed {MAX_TEXT_FILE_LINES} lines; split modules or add a reviewed exception: {}",
        violations.join(", ")
    );
}

fn reject_long_text_lines(repo: &Path, paths: &[PathBuf]) -> Result<()> {
    let mut violations = Vec::new();
    for relative in paths {
        let Some(text) = tracked_text(repo, relative)? else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            let width = line.chars().count();
            if width > MAX_TEXT_LINE_WIDTH {
                violations.push(format!(
                    "{}:{}:{width}>{MAX_TEXT_LINE_WIDTH}",
                    relative.display(),
                    index + 1
                ));
            }
        }
    }
    if violations.is_empty() {
        return Ok(());
    }
    bail!(
        "tracked text lines exceed {MAX_TEXT_LINE_WIDTH} characters; wrap prose/code or extract helpers: {}",
        violations.join(", ")
    );
}

fn reject_python_skill_scripts(repo: &Path, paths: &[PathBuf]) -> Result<()> {
    let mut violations = Vec::new();
    for relative in paths {
        if !is_skill_script_path(relative) {
            continue;
        }
        let text = tracked_text(repo, relative)?;
        if skill_script_uses_python(relative, text.as_deref()) {
            violations.push(relative.display().to_string());
        }
    }
    if violations.is_empty() {
        return Ok(());
    }
    bail!(
        "repo-local skill scripts must not use Python; use POSIX shell or Rust-owned tooling: {}",
        violations.join(", ")
    );
}

fn tracked_text(repo: &Path, relative: &Path) -> Result<Option<String>> {
    if ignored_tracked_path(relative) {
        return Ok(None);
    }
    let path = repo.join(relative);
    if path.is_dir() {
        return Ok(None);
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::IsADirectory => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", relative.display()));
        }
    };
    if bytes.contains(&0) {
        return Ok(None);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Some(text)),
        Err(_) => Ok(None),
    }
}

fn is_skill_script_path(relative: &Path) -> bool {
    let path = relative.to_string_lossy();
    path.starts_with(".codex/skills/") && path.contains("/scripts/")
}

fn skill_script_uses_python(relative: &Path, text: Option<&str>) -> bool {
    if !is_skill_script_path(relative) {
        return false;
    }
    if relative
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension == "py")
    {
        return true;
    }
    text.and_then(|text| text.lines().next())
        .is_some_and(|line| line.starts_with("#!") && line.contains("python"))
}

fn ignored_tracked_path(relative: &Path) -> bool {
    matches!(
        relative
            .extension()
            .and_then(|extension| extension.to_str()),
        Some(
            "jpg"
                | "jpeg"
                | "png"
                | "gif"
                | "webp"
                | "ico"
                | "zip"
                | "gz"
                | "xz"
                | "zst"
                | "wasm"
                | "woff"
                | "woff2"
        )
    )
}

fn large_file_allowed(relative: &Path) -> bool {
    let path = relative.to_string_lossy();
    LARGE_FILE_EXCEPTIONS
        .iter()
        .any(|exception| exception.path == path && !exception.reason.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_file_allowlist_is_explicit() {
        assert!(large_file_allowed(Path::new("Cargo.lock")));
        assert!(!large_file_allowed(Path::new("src/new_large_module.rs")));
    }

    #[test]
    fn binary_asset_extensions_are_ignored() {
        assert!(ignored_tracked_path(Path::new(
            "docs/screenshots/qcold-web-terminals.jpg"
        )));
        assert!(!ignored_tracked_path(Path::new("src/webapp.rs")));
    }

    #[test]
    fn python_skill_scripts_are_rejected() {
        assert!(skill_script_uses_python(
            Path::new(".codex/skills/repo-task-run-audit/scripts/helper.py"),
            Some("#!/usr/bin/env python3")
        ));
        assert!(skill_script_uses_python(
            Path::new(".codex/skills/repo-task-run-audit/scripts/helper"),
            Some("#!/usr/bin/python3")
        ));
        assert!(!skill_script_uses_python(
            Path::new(".codex/skills/repo-task-run-audit/scripts/helper.sh"),
            Some("#!/bin/sh")
        ));
        assert!(!skill_script_uses_python(
            Path::new("scripts/helper.py"),
            Some("#!/usr/bin/env python3")
        ));
    }

    #[test]
    fn pending_deleted_tracked_files_are_ignored() {
        assert!(
            tracked_text(Path::new("."), Path::new("definitely-missing.yml"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn gitlink_directories_are_ignored() {
        let root =
            std::env::temp_dir().join(format!("qcold-quality-gitlink-{}", std::process::id()));
        let path = root.join("submodule");
        fs::create_dir_all(&path).unwrap();

        assert!(tracked_text(&root, Path::new("submodule"))
            .unwrap()
            .is_none());

        fs::remove_dir_all(root).unwrap();
    }
}
