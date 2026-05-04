use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

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
    if let Some(id) = optional_env("QCOLD_ACTIVE_REPO") {
        return active_by_id(&id);
    }
    if let Some(root) = optional_env("QCOLD_REPO_ROOT") {
        return fallback_for_root(Path::new(&root));
    }

    let repos = list()?;
    if repos.is_empty() {
        return fallback();
    }
    if let Some(repo) = repos.iter().find(|repo| repo.active) {
        return Ok(repo.clone());
    }
    Ok(repos[0].clone())
}

pub fn active_root() -> Result<PathBuf> {
    Ok(active()?.root)
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
