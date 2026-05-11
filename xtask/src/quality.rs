use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

const MAX_TEXT_FILE_LINES: usize = 1_000;
const MAX_TEXT_LINE_WIDTH: usize = 120;

const LARGE_FILE_EXCEPTIONS: &[LargeFileException] = &[LargeFileException {
    path: "Cargo.lock",
    reason: "Cargo owns lockfile shape",
}];

struct LargeFileException {
    path: &'static str,
    reason: &'static str,
}

pub(crate) fn run(repo: &Path) -> Result<()> {
    let tracked = tracked_files(repo)?;
    reject_large_text_files(repo, &tracked)?;
    reject_long_text_lines(repo, &tracked)
}

fn tracked_files(repo: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
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

fn tracked_text(repo: &Path, relative: &Path) -> Result<Option<String>> {
    if ignored_tracked_path(relative) {
        return Ok(None);
    }
    let bytes = fs::read(repo.join(relative))
        .with_context(|| format!("failed to read {}", relative.display()))?;
    if bytes.contains(&0) {
        return Ok(None);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Some(text)),
        Err(_) => Ok(None),
    }
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
}
