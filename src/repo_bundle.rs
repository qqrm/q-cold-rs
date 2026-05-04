use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

pub fn run() -> Result<u8> {
    let repo = Repository::discover()?;
    let bundle = create_source_bundle(&repo)?;
    println!("BUNDLE_PATH={}", bundle.path.display());
    Ok(0)
}

fn create_source_bundle(repo: &Repository) -> Result<Bundle> {
    ensure_clean_worktree(&repo.root)?;
    let bundles_dir = repo.root.join("bundles");
    fs::create_dir_all(&bundles_dir)
        .with_context(|| format!("failed to create {}", bundles_dir.display()))?;

    let archive_name = format!("{}-{}-source.zip", repo.name, repo.short_head);
    let archive = bundles_dir.join(archive_name);
    let prefix = format!("{}-{}/", repo.name, repo.short_head);
    let manifest_path = format!("{prefix}metadata/bundle-manifest.txt");
    let manifest = manifest_content(repo, &archive, &manifest_path);

    let status = Command::new("git")
        .current_dir(&repo.root)
        .args([
            "archive",
            "--format=zip",
            &format!("--prefix={prefix}"),
            &format!("--add-virtual-file={manifest_path}:{manifest}"),
            "-o",
        ])
        .arg(&archive)
        .arg("HEAD")
        .status()
        .context("failed to run git archive")?;
    if !status.success() {
        bail!("git archive failed with status {status}");
    }

    Ok(Bundle { path: archive })
}

fn ensure_clean_worktree(root: &Path) -> Result<()> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain"])
        .output()
        .context("failed to inspect git status")?;
    if !output.status.success() {
        bail!("git status failed with status {}", output.status);
    }
    if !output.stdout.is_empty() {
        bail!(
            "repository has uncommitted changes; commit or stash them before creating a source bundle"
        );
    }
    Ok(())
}

fn manifest_content(repo: &Repository, archive: &Path, manifest_path: &str) -> String {
    let created_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format!(
        "repo={}\nbranch={}\ncommit={}\ncreated_unix={}\narchive={}\narchive_format=zip\nmanifest={}\n",
        repo.name,
        repo.branch,
        repo.head,
        created_unix,
        archive.display(),
        manifest_path
    )
}

struct Repository {
    root: PathBuf,
    name: String,
    branch: String,
    head: String,
    short_head: String,
}

impl Repository {
    fn discover() -> Result<Self> {
        let root = PathBuf::from(git_output(None, &["rev-parse", "--show-toplevel"])?);
        Self::from_root(&root)
    }

    fn from_root(root: &Path) -> Result<Self> {
        let root = PathBuf::from(git_output(Some(root), &["rev-parse", "--show-toplevel"])?);
        let name = root
            .file_name()
            .and_then(|value| value.to_str())
            .context("repository root has no valid final path component")?
            .to_string();
        Ok(Self {
            branch: git_output(Some(&root), &["branch", "--show-current"])
                .unwrap_or_else(|_| "detached".to_string()),
            head: git_output(Some(&root), &["rev-parse", "HEAD"])?,
            short_head: git_output(Some(&root), &["rev-parse", "--short=12", "HEAD"])?,
            root,
            name,
        })
    }
}

struct Bundle {
    path: PathBuf,
}

fn git_output(current_dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    if let Some(dir) = current_dir {
        command.current_dir(dir);
    }
    let output = command
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed with status {}",
            args.join(" "),
            output.status
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::tempdir;

    use super::{create_source_bundle, Repository};

    #[test]
    fn source_bundle_writes_single_zip_with_embedded_manifest_under_bundles() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        git(root, &["init"]);
        git(root, &["config", "user.email", "qcold@example.test"]);
        git(root, &["config", "user.name", "Q-COLD Test"]);
        fs::write(root.join("README.md"), "fixture\n").unwrap();
        git(root, &["add", "README.md"]);
        git(root, &["commit", "-m", "seed"]);

        let repo = Repository::from_root(root).unwrap();
        let bundle = create_source_bundle(&repo).unwrap();

        assert!(bundle.path.starts_with(root.join("bundles")));
        assert!(bundle.path.exists());
        assert_eq!(bundle.path.extension().unwrap(), "zip");
        assert_eq!(fs::read_dir(root.join("bundles")).unwrap().count(), 1);

        let manifest = unzip_stdout(&bundle.path, "metadata/bundle-manifest.txt");
        assert!(manifest.contains("repo="));
        assert!(manifest.contains("archive_format=zip"));
    }

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {} failed", args.join(" "));
    }

    fn unzip_stdout(archive: &Path, needle: &str) -> String {
        let listing = Command::new("unzip")
            .args(["-Z1"])
            .arg(archive)
            .output()
            .unwrap();
        assert!(listing.status.success(), "unzip listing failed");
        let entry = String::from_utf8_lossy(&listing.stdout)
            .lines()
            .find(|line| line.ends_with(needle))
            .unwrap()
            .to_string();
        let output = Command::new("unzip")
            .args(["-p"])
            .arg(archive)
            .arg(entry)
            .output()
            .unwrap();
        assert!(output.status.success(), "unzip extract failed");
        String::from_utf8_lossy(&output.stdout).to_string()
    }
}
