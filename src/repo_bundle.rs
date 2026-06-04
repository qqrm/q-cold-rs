use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

pub fn run() -> Result<u8> {
    let root = Repository::discover_root()?;
    let bundle = create_source_bundle(&root)?;
    println!("BUNDLE_PATH={}", bundle.path.display());
    Ok(0)
}

fn create_source_bundle(root: &Path) -> Result<Bundle> {
    let root = Repository::canonical_root(root)?;
    sync_clean_checkout_to_upstream(&root)?;
    let repo = Repository::from_root(&root)?;
    let bundles_dir = repo.root.join("bundles");
    fs::create_dir_all(&bundles_dir)
        .with_context(|| format!("failed to create {}", bundles_dir.display()))?;

    let archive_name = format!("{}-{}-source.zip", repo.name, repo.short_head);
    let archive = bundles_dir.join(archive_name);
    let prefix = format!("{}-{}/", repo.name, repo.short_head);
    let manifest_path = format!("{prefix}metadata/bundle-manifest.txt");
    let manifest = manifest_content(&repo, &archive, &manifest_path);
    let summary_path = format!("{prefix}summary.md");
    let summary = summary_content(&repo, &archive, &manifest_path);

    let status = Command::new("git")
        .current_dir(&repo.root)
        .args([
            "archive",
            "--format=zip",
            &format!("--prefix={prefix}"),
            &format!("--add-virtual-file={manifest_path}:{manifest}"),
            &format!("--add-virtual-file={summary_path}:{summary}"),
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

fn sync_clean_checkout_to_upstream(root: &Path) -> Result<()> {
    ensure_clean_worktree(root)?;
    let Some(remote) = git_output_optional(
        Some(root),
        &[
            "config",
            "--get",
            &format!(
                "branch.{}.remote",
                git_output(Some(root), &["branch", "--show-current"])?
            ),
        ],
    )?
    .filter(|remote| !remote.is_empty())
    else {
        eprintln!("warning: source bundle branch has no configured upstream; archiving current HEAD");
        return Ok(());
    };
    if remote != "." {
        run_git(root, &["fetch", remote.as_str()]).context("source bundle preflight fetch failed")?;
    }
    run_git(root, &["merge", "--ff-only", "@{upstream}"])
        .context("source bundle preflight fast-forward failed")?;
    ensure_clean_worktree(root)?;
    Ok(())
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

fn summary_content(repo: &Repository, archive: &Path, manifest_path: &str) -> String {
    format!(
        "# Q-COLD Source Bundle\n\n\
         - Repository: `{}`\n\
         - Branch: `{}`\n\
         - Commit: `{}`\n\
         - Archive: `{}`\n\
         - Metadata: `{}`\n\n\
         Machine-readable bundle metadata lives in `{}`.\n",
        repo.name,
        repo.branch,
        repo.head,
        archive.display(),
        manifest_path,
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
    fn discover_root() -> Result<PathBuf> {
        Ok(PathBuf::from(git_output(
            None,
            &["rev-parse", "--show-toplevel"],
        )?))
    }

    fn from_root(root: &Path) -> Result<Self> {
        let root = Self::canonical_root(root)?;
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

    fn canonical_root(root: &Path) -> Result<PathBuf> {
        Ok(PathBuf::from(git_output(
            Some(root),
            &["rev-parse", "--show-toplevel"],
        )?))
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

fn git_output_optional(current_dir: Option<&Path>, args: &[&str]) -> Result<Option<String>> {
    let mut command = Command::new("git");
    if let Some(dir) = current_dir {
        command.current_dir(dir);
    }
    let output = command
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

fn run_git(root: &Path, args: &[&str]) -> Result<()> {
    let display = args.join(" ");
    let status = Command::new("git")
        .current_dir(root)
        .args(args)
        .status()
        .with_context(|| format!("failed to run git {display}"))?;
    if !status.success() {
        bail!("git {display} failed with status {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use tempfile::tempdir;

    use super::create_source_bundle;

    #[test]
    fn source_bundle_writes_single_zip_with_embedded_manifest_under_bundles() {
        let temp = tempdir().unwrap();
        let root = seed_tracking_repo(temp.path());
        let bundle = create_source_bundle(&root).unwrap();

        assert!(bundle.path.starts_with(root.join("bundles")));
        assert!(bundle.path.exists());
        assert_eq!(bundle.path.extension().unwrap(), "zip");
        assert_eq!(fs::read_dir(root.join("bundles")).unwrap().count(), 1);

        let manifest = unzip_stdout(&bundle.path, "metadata/bundle-manifest.txt");
        assert!(manifest.contains("repo="));
        assert!(manifest.contains("archive_format=zip"));
        let summary = unzip_stdout(&bundle.path, "summary.md");
        assert!(summary.contains("# Q-COLD Source Bundle"));
        assert!(summary.contains("Machine-readable bundle metadata lives in"));
    }

    #[test]
    fn source_bundle_without_upstream_archives_current_head() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "--initial-branch=main"]);
        configure_identity(root);
        fs::write(root.join("README.md"), "fixture\n").unwrap();
        git(root, &["add", "README.md"]);
        git(root, &["commit", "-m", "seed"]);

        let bundle = create_source_bundle(root).unwrap();

        assert!(bundle.path.exists());
        assert!(unzip_stdout(&bundle.path, "README.md").contains("fixture"));
    }

    #[test]
    fn source_bundle_fast_forwards_from_upstream_before_archiving() {
        let temp = tempdir().unwrap();
        let root = seed_tracking_repo(temp.path());
        let peer = temp.path().join("peer");
        git_clone(&temp.path().join("remote.git"), &peer);
        configure_identity(&peer);
        fs::write(peer.join("remote.txt"), "remote\n").unwrap();
        git(&peer, &["add", "remote.txt"]);
        git(&peer, &["commit", "-m", "advance"]);
        git(&peer, &["push", "origin", "main"]);

        let bundle = create_source_bundle(&root).unwrap();

        assert_eq!(
            git_stdout(&root, &["rev-parse", "HEAD"]),
            git_stdout(&peer, &["rev-parse", "HEAD"])
        );
        assert!(unzip_stdout(&bundle.path, "remote.txt").contains("remote"));
    }

    fn seed_tracking_repo(temp: &Path) -> PathBuf {
        let remote = temp.join("remote.git");
        let root = temp.join("repo");
        git_init_bare(&remote);
        git_clone(&remote, &root);
        configure_identity(&root);
        fs::write(root.join("README.md"), "fixture\n").unwrap();
        git(&root, &["add", "README.md"]);
        git(&root, &["commit", "-m", "seed"]);
        git(&root, &["push", "-u", "origin", "main"]);
        root
    }

    fn git_init_bare(path: &Path) {
        let status = Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success(), "git init --bare failed");
    }

    fn git_clone(remote: &Path, dest: &Path) {
        let status = Command::new("git")
            .args(["clone"])
            .arg(remote)
            .arg(dest)
            .status()
            .unwrap();
        assert!(status.success(), "git clone failed");
    }

    fn configure_identity(root: &Path) {
        git(root, &["config", "user.email", "qcold@example.test"]);
        git(root, &["config", "user.name", "Q-COLD Test"]);
    }

    fn git_stdout(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {} failed", args.join(" "));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
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
