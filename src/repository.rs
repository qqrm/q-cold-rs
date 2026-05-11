use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Clone, Copy)]
pub enum AdapterContext {
    ActiveRepository,
    CwdManagedWorktree,
}

#[derive(Args)]
pub struct RepositoryArgs {
    #[command(subcommand)]
    command: RepositoryCommand,
}

#[derive(Subcommand)]
enum RepositoryCommand {
    #[command(about = "List registered repository connections")]
    List,
    #[command(about = "Register or update a repository connection")]
    Add(AddRepositoryArgs),
    #[command(about = "Remove a repository connection")]
    Remove { id: String },
    #[command(about = "Select the active repository for daemon and adapter-backed commands")]
    SetActive { id: String },
    #[command(about = "Show the active repository")]
    Current,
}

#[derive(Args)]
struct AddRepositoryArgs {
    id: String,
    root: PathBuf,
    #[arg(long, default_value = "xtask-process")]
    adapter: String,
    #[arg(long)]
    xtask_manifest: Option<PathBuf>,
    #[arg(long)]
    default_branch: Option<String>,
    #[arg(long)]
    set_active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RepositoryConfig {
    pub id: String,
    pub root: PathBuf,
    pub adapter: String,
    pub xtask_manifest: Option<PathBuf>,
    pub default_branch: Option<String>,
    pub active: bool,
}

pub fn run(args: RepositoryArgs) -> Result<u8> {
    match args.command {
        RepositoryCommand::List => print!("{}", snapshot()?),
        RepositoryCommand::Add(args) => {
            let repo = RepositoryConfig {
                id: args.id,
                root: canonical_root(&args.root)?,
                adapter: args.adapter,
                xtask_manifest: args
                    .xtask_manifest
                    .map(|path| canonical_existing(&path))
                    .transpose()?,
                default_branch: args.default_branch,
                active: args.set_active,
            };
            upsert(&repo)?;
            if args.set_active {
                set_active(&repo.id)?;
            }
            println!("{}", render_repo(&active_by_id(&repo.id)?));
        }
        RepositoryCommand::Remove { id } => remove(&id)?,
        RepositoryCommand::SetActive { id } => {
            set_active(&id)?;
            println!("{}", render_repo(&active()?));
        }
        RepositoryCommand::Current => println!("{}", render_repo(&active()?)),
    }
    Ok(0)
}

pub fn snapshot() -> Result<String> {
    let repos = list()?;
    if repos.is_empty() {
        return Ok(format!(
            "repositories\tcount=0\n{}\n",
            render_repo(&fallback()?)
        ));
    }
    let mut lines = vec![format!("repositories\tcount={}", repos.len())];
    lines.extend(repos.iter().map(render_repo));
    Ok(format!("{}\n", lines.join("\n")))
}

pub fn list() -> Result<Vec<RepositoryConfig>> {
    let connection = open_db()?;
    let active_id = active_id(&connection)?;
    let mut statement = connection
        .prepare(
            "select id, root, adapter, xtask_manifest, default_branch
             from repositories
             order by id",
        )
        .context("failed to prepare repository query")?;
    let rows = statement
        .query_map([], |row| {
            let id: String = row.get(0)?;
            Ok(RepositoryConfig {
                active: active_id.as_deref() == Some(id.as_str()),
                id,
                root: PathBuf::from(row.get::<_, String>(1)?),
                adapter: row.get(2)?,
                xtask_manifest: row.get::<_, Option<String>>(3)?.map(PathBuf::from),
                default_branch: row.get(4)?,
            })
        })
        .context("failed to query repositories")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode repository rows")?;
    Ok(rows)
}

pub fn active() -> Result<RepositoryConfig> {
    Ok(resolve_unchecked(AdapterContext::ActiveRepository)?.repo)
}

/// Resolve a repository for adapter-backed command execution.
///
/// This path rejects mismatches between the caller's git checkout and the
/// resolved repository target. Read-only registry views use `active()` so they
/// remain available even when an operator needs to inspect or repair state.
pub fn for_adapter_context(context: AdapterContext) -> Result<RepositoryConfig> {
    let resolved = resolve_unchecked(context)?;
    ensure_cwd_matches_resolved_repo(&resolved.repo, context, &resolved.source)?;
    Ok(resolved.repo)
}

struct ResolvedRepository {
    repo: RepositoryConfig,
    source: String,
}

fn resolve_unchecked(context: AdapterContext) -> Result<ResolvedRepository> {
    if let Some(root) = optional_env("QCOLD_REPO_ROOT") {
        let root = canonical_root(Path::new(&root))?;
        if root.join(".task/task.env").is_file() {
            let repo = match context {
                AdapterContext::ActiveRepository => config_for_managed_worktree_primary(&root)?,
                AdapterContext::CwdManagedWorktree => config_for_managed_worktree(&root)?,
            };
            return Ok(ResolvedRepository {
                repo,
                source: format!("QCOLD_REPO_ROOT={}", root.display()),
            });
        }
        if matches!(context, AdapterContext::CwdManagedWorktree) {
            if let Some(cwd_root) = cwd_managed_worktree_root()? {
                if primary_repo_path(&cwd_root)?.as_deref() == Some(root.as_path()) {
                    return Ok(ResolvedRepository {
                        repo: config_for_managed_worktree(&cwd_root)?,
                        source: format!("cwd managed worktree {}", cwd_root.display()),
                    });
                }
            }
        }
        return Ok(ResolvedRepository {
            repo: fallback_for_root(&root)?,
            source: format!("QCOLD_REPO_ROOT={}", root.display()),
        });
    }
    if let Some(id) = optional_env("QCOLD_ACTIVE_REPO") {
        return Ok(ResolvedRepository {
            repo: active_by_id(&id)?,
            source: format!("QCOLD_ACTIVE_REPO={id}"),
        });
    }
    if matches!(context, AdapterContext::CwdManagedWorktree) {
        if let Some(root) = cwd_managed_worktree_root()? {
            return Ok(ResolvedRepository {
                repo: config_for_managed_worktree(&root)?,
                source: format!("cwd managed worktree {}", root.display()),
            });
        }
    }

    let repos = list()?;
    if repos.is_empty() {
        if let Some(root) = cwd_managed_worktree_root()? {
            return Ok(ResolvedRepository {
                repo: config_for_managed_worktree_primary(&root)?,
                source: format!("cwd managed worktree {}", root.display()),
            });
        }
        return Ok(ResolvedRepository {
            repo: fallback()?,
            source: "current checkout fallback".to_string(),
        });
    }
    if let Some(repo) = repos.iter().find(|repo| repo.active) {
        return Ok(ResolvedRepository {
            repo: repo.clone(),
            source: format!("active repository {}", repo.id),
        });
    }
    Ok(ResolvedRepository {
        repo: repos[0].clone(),
        source: format!("first registered repository {}", repos[0].id),
    })
}

fn ensure_cwd_matches_resolved_repo(
    repo: &RepositoryConfig,
    context: AdapterContext,
    source: &str,
) -> Result<()> {
    let cwd_root = match git_root() {
        Ok(root) => canonical_root(&root)?,
        Err(_) => return Ok(()),
    };
    if cwd_root == repo.root {
        return Ok(());
    }

    let cwd_primary = if cwd_root.join(".task/task.env").is_file() {
        primary_repo_path(&cwd_root)?
    } else {
        None
    };
    match context {
        AdapterContext::ActiveRepository if cwd_primary.as_deref() == Some(repo.root.as_path()) => {
            return Ok(());
        }
        AdapterContext::CwdManagedWorktree if repo.root == cwd_root => return Ok(()),
        _ => {}
    }

    bail!(
        "repository target mismatch: cwd git root is {}; resolved target root is {}; source is {source}; \
         mutating or task-flow-sensitive commands must run from the target checkout or one of its managed worktrees",
        cwd_root.display(),
        repo.root.display(),
    )
}

pub fn active_root() -> Result<PathBuf> {
    Ok(active()?.root)
}

pub fn current_or_active() -> Result<RepositoryConfig> {
    if let Some(repo) = current_checkout_config()? {
        return Ok(repo);
    }
    // This fallback is intentionally unguarded for read-oriented callers. Do
    // not use it to dispatch mutating adapter work.
    active()
}

pub fn current_or_active_root() -> Result<PathBuf> {
    Ok(current_or_active()?.root)
}

fn current_checkout_config() -> Result<Option<RepositoryConfig>> {
    let root = match git_root() {
        Ok(root) => canonical_root(&root)?,
        Err(_) => return Ok(None),
    };
    current_checkout_config_for_root(&root).map(Some)
}

fn current_checkout_config_for_root(root: &Path) -> Result<RepositoryConfig> {
    let root = canonical_root(root)?;
    if root.join(".task/task.env").is_file() {
        return config_for_managed_worktree_primary(&root);
    }
    if let Some(mut repo) = registered_for_root(&root)? {
        repo.active = true;
        return Ok(repo);
    }
    fallback_for_root(&root)
}

fn cwd_managed_worktree_root() -> Result<Option<PathBuf>> {
    match git_root() {
        Ok(root) if root.join(".task/task.env").is_file() => Ok(Some(canonical_root(&root)?)),
        Ok(_) | Err(_) => Ok(None),
    }
}

fn config_for_managed_worktree(root: &Path) -> Result<RepositoryConfig> {
    let root = canonical_root(root)?;
    if let Some(primary) = primary_repo_path(&root)? {
        if let Some(mut repo) = registered_for_root(&primary)? {
            repo.id = format!("{}:worktree", repo.id);
            repo.root = root;
            repo.active = true;
            return Ok(repo);
        }
    }
    fallback_for_root(&root)
}

fn config_for_managed_worktree_primary(root: &Path) -> Result<RepositoryConfig> {
    let root = canonical_root(root)?;
    let Some(primary) = primary_repo_path(&root)? else {
        return fallback_for_root(&root);
    };
    if let Some(mut repo) = registered_for_root(&primary)? {
        repo.active = true;
        return Ok(repo);
    }
    fallback_for_root(&primary)
}

fn registered_for_root(root: &Path) -> Result<Option<RepositoryConfig>> {
    let root = canonical_root(root)?;
    Ok(list()?.into_iter().find(|repo| repo.root == root))
}

pub fn upsert(repo: &RepositoryConfig) -> Result<()> {
    if repo.id.trim().is_empty() {
        bail!("repository id is empty");
    }
    if repo.adapter.trim().is_empty() {
        bail!("repository adapter is empty");
    }
    if !repo.root.is_dir() {
        bail!("repository root does not exist: {}", repo.root.display());
    }
    let connection = open_db()?;
    let now = unix_now();
    connection
        .execute(
            "insert into repositories
                 (id, root, adapter, xtask_manifest, default_branch, created_at, updated_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             on conflict(id) do update set
                 root=excluded.root,
                 adapter=excluded.adapter,
                 xtask_manifest=excluded.xtask_manifest,
                 default_branch=excluded.default_branch,
                 updated_at=excluded.updated_at",
            params![
                repo.id,
                repo.root.display().to_string(),
                repo.adapter,
                repo.xtask_manifest
                    .as_ref()
                    .map(|path| path.display().to_string()),
                repo.default_branch,
                now,
            ],
        )
        .context("failed to upsert repository")?;
    Ok(())
}

fn active_by_id(id: &str) -> Result<RepositoryConfig> {
    list()?
        .into_iter()
        .find(|repo| repo.id == id)
        .with_context(|| format!("unknown repository id: {id}"))
}

fn remove(id: &str) -> Result<()> {
    let connection = open_db()?;
    connection
        .execute("delete from repositories where id = ?1", [id])
        .context("failed to remove repository")?;
    if active_id(&connection)?.as_deref() == Some(id) {
        connection
            .execute("delete from settings where key = 'active_repository'", [])
            .context("failed to clear active repository")?;
    }
    Ok(())
}

fn set_active(id: &str) -> Result<()> {
    let connection = open_db()?;
    let exists = connection
        .query_row("select 1 from repositories where id = ?1", [id], |_| Ok(()))
        .optional()
        .context("failed to inspect repository")?
        .is_some();
    if !exists {
        bail!("unknown repository id: {id}");
    }
    connection
        .execute(
            "insert into settings (key, value) values ('active_repository', ?1)
             on conflict(key) do update set value=excluded.value",
            [id],
        )
        .context("failed to set active repository")?;
    Ok(())
}

fn active_id(connection: &Connection) -> Result<Option<String>> {
    connection
        .query_row(
            "select value from settings where key = 'active_repository'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to read active repository")
}

fn render_repo(repo: &RepositoryConfig) -> String {
    format!(
        "repository\t{}\troot={}\tadapter={}\tactive={}\tbranch={}\txtask_manifest={}",
        repo.id,
        repo.root.display(),
        repo.adapter,
        if repo.active { "yes" } else { "no" },
        repo.default_branch.as_deref().unwrap_or(""),
        repo.xtask_manifest
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
    )
}

fn fallback() -> Result<RepositoryConfig> {
    let root = git_root().unwrap_or(env::current_dir().context("failed to read cwd")?);
    fallback_for_root(&root)
}

fn fallback_for_root(root: &Path) -> Result<RepositoryConfig> {
    let root = canonical_root(root)?;
    Ok(RepositoryConfig {
        id: root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("repository")
            .to_string(),
        root,
        adapter: "xtask-process".to_string(),
        xtask_manifest: optional_env("QCOLD_XTASK_MANIFEST").map(PathBuf::from),
        default_branch: None,
        active: true,
    })
}

fn primary_repo_path(root: &Path) -> Result<Option<PathBuf>> {
    let task_env = root.join(".task/task.env");
    let contents = match std::fs::read_to_string(&task_env) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", task_env.display()))
        }
    };
    let Some(raw) = contents
        .lines()
        .find_map(|line| line.strip_prefix("PRIMARY_REPO_PATH="))
    else {
        return Ok(None);
    };
    let value = unquote_env_value(raw);
    if value.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(canonical_root(Path::new(&value))?))
}

fn unquote_env_value(raw: &str) -> String {
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        raw[1..raw.len() - 1].replace("'\\''", "'")
    } else {
        raw.to_string()
    }
}

fn canonical_root(root: &Path) -> Result<PathBuf> {
    root.canonicalize()
        .with_context(|| format!("failed to resolve repository root {}", root.display()))
}

fn canonical_existing(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))
}

fn git_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to inspect git root")?;
    if !output.status.success() {
        bail!("current directory is not inside a git checkout");
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

fn open_db() -> Result<Connection> {
    let path = db_path()?;
    std::fs::create_dir_all(
        path.parent()
            .context("repository db path has no parent directory")?,
    )?;
    let connection =
        Connection::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
    connection
        .execute_batch(
            "pragma journal_mode = wal;
             create table if not exists repositories (
                 id text primary key,
                 root text not null,
                 adapter text not null,
                 xtask_manifest text,
                 default_branch text,
                 created_at integer not null,
                 updated_at integer not null
             );
             create table if not exists settings (
                 key text primary key,
                 value text not null
             );",
        )
        .context("failed to initialize repository registry")?;
    Ok(connection)
}

fn db_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("qcold.sqlite3"))
}

fn state_dir() -> Result<PathBuf> {
    if let Ok(path) = env::var("QCOLD_STATE_DIR") {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    let home = env::var("HOME").context("HOME is required when QCOLD_STATE_DIR is unset")?;
    Ok(PathBuf::from(home).join(".local/state/qcold"))
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_worktree_config_reuses_registered_primary_adapter_settings() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        let original_cwd = env::current_dir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));
        env::set_current_dir(temp.path()).unwrap();

        let primary = temp.path().join("primary");
        let worktree = temp.path().join("WT/primary/anchor-task");
        let manifest = temp.path().join("fixtures/xtask/Cargo.toml");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(worktree.join(".task")).unwrap();
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(&manifest, "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(
            worktree.join(".task/task.env"),
            format!("PRIMARY_REPO_PATH='{}'\n", primary.display()),
        )
        .unwrap();

        upsert(&RepositoryConfig {
            id: "primary".to_string(),
            root: canonical_root(&primary).unwrap(),
            adapter: "xtask-process".to_string(),
            xtask_manifest: Some(canonical_existing(&manifest).unwrap()),
            default_branch: Some("developer".to_string()),
            active: true,
        })
        .unwrap();

        let repo = config_for_managed_worktree(&worktree).unwrap();
        assert_eq!(repo.id, "primary:worktree");
        assert_eq!(repo.root, canonical_root(&worktree).unwrap());
        assert_eq!(repo.xtask_manifest, Some(canonical_existing(&manifest).unwrap()));
        assert_eq!(repo.default_branch.as_deref(), Some("developer"));
        assert!(repo.active);

        env::set_var("QCOLD_REPO_ROOT", &worktree);
        let repo = for_adapter_context(AdapterContext::ActiveRepository).unwrap();
        assert_eq!(repo.root, canonical_root(&primary).unwrap());
        assert_eq!(repo.xtask_manifest, Some(canonical_existing(&manifest).unwrap()));

        let repo = for_adapter_context(AdapterContext::CwdManagedWorktree).unwrap();
        assert_eq!(repo.root, canonical_root(&worktree).unwrap());
        assert_eq!(repo.xtask_manifest, Some(canonical_existing(&manifest).unwrap()));

        env::remove_var("QCOLD_REPO_ROOT");
        env::remove_var("QCOLD_STATE_DIR");
        env::set_current_dir(original_cwd).unwrap();
    }

    #[test]
    fn active_fallback_from_managed_worktree_uses_primary_root() {
        let _guard = crate::test_support::env_guard();
        let temp = tempfile::tempdir().unwrap();
        let original_cwd = env::current_dir().unwrap();
        env::set_var("QCOLD_STATE_DIR", temp.path().join("state"));

        let primary = temp.path().join("primary");
        let worktree = temp.path().join("WT/primary/anchor-task");
        std::fs::create_dir_all(primary.join("xtask")).unwrap();
        std::fs::create_dir_all(worktree.join(".task")).unwrap();
        let status = Command::new("git")
            .arg("init")
            .current_dir(&worktree)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::write(
            primary.join("xtask/Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            worktree.join(".task/task.env"),
            format!("PRIMARY_REPO_PATH='{}'\n", primary.display()),
        )
        .unwrap();

        env::set_current_dir(&worktree).unwrap();
        let repo = for_adapter_context(AdapterContext::ActiveRepository).unwrap();
        assert_eq!(repo.root, canonical_root(&primary).unwrap());

        let repo = for_adapter_context(AdapterContext::CwdManagedWorktree).unwrap();
        assert_eq!(repo.root, canonical_root(&worktree).unwrap());

        env::set_var("QCOLD_REPO_ROOT", &primary);
        let repo = for_adapter_context(AdapterContext::ActiveRepository).unwrap();
        assert_eq!(repo.root, canonical_root(&primary).unwrap());

        let repo = for_adapter_context(AdapterContext::CwdManagedWorktree).unwrap();
        assert_eq!(repo.root, canonical_root(&worktree).unwrap());
        env::remove_var("QCOLD_REPO_ROOT");

        env::set_current_dir(original_cwd).unwrap();
        env::remove_var("QCOLD_STATE_DIR");
    }

    #[test]
    fn env_value_parser_handles_single_quoted_paths() {
        assert_eq!(unquote_env_value("plain"), "plain");
        assert_eq!(unquote_env_value("'with spaces'"), "with spaces");
        assert_eq!(unquote_env_value("'it'\\''s'"), "it's");
    }
}
