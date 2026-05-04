use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub timestamp: u64,
    pub source: String,
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct AgentRow {
    pub id: String,
    pub track: String,
    pub pid: u32,
    pub started_at: u64,
    pub command: Vec<String>,
    pub stdout_log_path: Option<PathBuf>,
    pub stderr_log_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct TaskTopicRow {
    pub id: String,
    pub chat_id: String,
    pub thread_id: i64,
    pub title: String,
    pub description: String,
    pub source_message_id: i64,
    pub created_at: u64,
    pub status: String,
    pub topic_name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TerminalMetadataRow {
    pub target: String,
    pub name: Option<String>,
    pub scope: Option<String>,
    pub updated_at: u64,
}

pub fn append_history(source: &str, role: &str, text: &str) -> Result<()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let connection = open_db()?;
    backfill_history(&connection)?;
    connection
        .execute(
            "insert into history (timestamp_unix, source, role, text) values (?1, ?2, ?3, ?4)",
            params![unix_now(), source, role, text],
        )
        .context("failed to insert history message")?;
    Ok(())
}

pub fn load_history(limit: usize) -> Result<Vec<HistoryEntry>> {
    let connection = open_db()?;
    backfill_history(&connection)?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut statement = connection
        .prepare(
            "select id, timestamp_unix, source, role, text
             from history
             order by id desc
             limit ?1",
        )
        .context("failed to prepare history query")?;
    let mut entries = statement
        .query_map([limit], |row| {
            Ok(HistoryEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                source: row.get(2)?,
                role: row.get(3)?,
                text: row.get(4)?,
            })
        })
        .context("failed to query history")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode history rows")?;
    entries.reverse();
    Ok(entries)
}

pub fn load_history_for_source(source: &str, limit: usize) -> Result<Vec<HistoryEntry>> {
    let connection = open_db()?;
    backfill_history(&connection)?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut statement = connection
        .prepare(
            "select id, timestamp_unix, source, role, text
             from history
             where source = ?1
             order by id desc
             limit ?2",
        )
        .context("failed to prepare source history query")?;
    let mut entries = statement
        .query_map(params![source, limit], |row| {
            Ok(HistoryEntry {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                source: row.get(2)?,
                role: row.get(3)?,
                text: row.get(4)?,
            })
        })
        .context("failed to query source history")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode source history rows")?;
    entries.reverse();
    Ok(entries)
}

pub fn load_agents(legacy_path: &Path) -> Result<Vec<AgentRow>> {
    let connection = open_db()?;
    backfill_agents(&connection, legacy_path)?;
    let mut statement = connection
        .prepare(
            "select id, track, pid, started_at_unix, command_json, stdout_log_path, stderr_log_path
             from agents
             order by started_at_unix, id",
        )
        .context("failed to prepare agent query")?;
    let rows = statement
        .query_map([], |row| {
            let command_json: String = row.get(4)?;
            let command = serde_json::from_str(&command_json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            Ok(AgentRow {
                id: row.get(0)?,
                track: row.get(1)?,
                pid: u32::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                started_at: row.get(3)?,
                command,
                stdout_log_path: row.get::<_, Option<String>>(5)?.map(PathBuf::from),
                stderr_log_path: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
            })
        })
        .context("failed to query agents")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode agent rows")?;
    Ok(rows)
}

pub fn insert_agent(agent: &AgentRow) -> Result<()> {
    let connection = open_db()?;
    connection
        .execute(
            "insert into agents
                 (id, track, pid, started_at_unix, command_json, stdout_log_path, stderr_log_path, created_at_unix)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?4)",
            params![
                agent.id,
                agent.track,
                agent.pid,
                agent.started_at,
                serde_json::to_string(&agent.command)?,
                agent.stdout_log_path.as_ref().map(|path| path.display().to_string()),
                agent.stderr_log_path.as_ref().map(|path| path.display().to_string()),
            ],
        )
        .context("failed to insert agent")?;
    Ok(())
}

pub fn load_terminal_metadata() -> Result<Vec<TerminalMetadataRow>> {
    let connection = open_db()?;
    let mut statement = connection
        .prepare(
            "select target, name, scope, updated_at_unix
             from terminal_metadata
             order by updated_at_unix, target",
        )
        .context("failed to prepare terminal metadata query")?;
    let rows = statement
        .query_map([], |row| {
            Ok(TerminalMetadataRow {
                target: row.get(0)?,
                name: row.get(1)?,
                scope: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })
        .context("failed to query terminal metadata")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode terminal metadata rows")?;
    Ok(rows)
}

pub fn save_terminal_metadata(target: &str, name: Option<&str>, scope: Option<&str>) -> Result<()> {
    let connection = open_db()?;
    if name.is_none() && scope.is_none() {
        connection
            .execute("delete from terminal_metadata where target = ?1", [target])
            .context("failed to clear terminal metadata")?;
        return Ok(());
    }
    connection
        .execute(
            "insert into terminal_metadata (target, name, scope, updated_at_unix)
             values (?1, ?2, ?3, ?4)
             on conflict(target) do update set
                 name = excluded.name,
                 scope = excluded.scope,
                 updated_at_unix = excluded.updated_at_unix",
            params![target, name, scope, unix_now()],
        )
        .context("failed to save terminal metadata")?;
    Ok(())
}

pub fn load_task_topics(legacy_path: &Path, legacy_events_dir: &Path) -> Result<Vec<TaskTopicRow>> {
    let connection = open_db()?;
    backfill_task_topics(&connection, legacy_path)?;
    backfill_task_events(&connection, legacy_events_dir)?;
    let mut statement = connection
        .prepare(
            "select t.id, tt.chat_id, tt.thread_id, t.title, t.description,
                    tt.source_message_id, t.created_at_unix, t.status, tt.topic_name
             from tasks t
             join task_topics tt on tt.task_id = t.id
             order by t.created_at_unix, t.id",
        )
        .context("failed to prepare task topic query")?;
    let rows = statement
        .query_map([], task_topic_from_row)
        .context("failed to query task topics")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode task topic rows")?;
    Ok(rows)
}

pub fn add_task_topic(record: &TaskTopicRow) -> Result<()> {
    let mut connection = open_db()?;
    let tx = connection
        .transaction()
        .context("failed to start task topic transaction")?;
    tx.execute(
        "insert into tasks
             (id, source, title, description, status, created_at_unix, updated_at_unix)
         values (?1, 'telegram', ?2, ?3, ?4, ?5, ?5)",
        params![
            record.id,
            record.title,
            record.description,
            record.status,
            record.created_at,
        ],
    )
    .context("failed to insert task")?;
    tx.execute(
        "insert into task_topics
             (task_id, chat_id, thread_id, topic_name, source_message_id)
         values (?1, ?2, ?3, ?4, ?5)",
        params![
            record.id,
            record.chat_id,
            record.thread_id,
            record.topic_name,
            record.source_message_id,
        ],
    )
    .context("failed to insert task topic")?;
    tx.commit().context("failed to commit task topic")?;
    Ok(())
}

pub fn append_event(
    source: &str,
    kind: &str,
    task_id: Option<&str>,
    agent_id: Option<&str>,
    run_id: Option<&str>,
    text: &str,
) -> Result<()> {
    let connection = open_db()?;
    connection
        .execute(
            "insert into events (timestamp_unix, source, kind, task_id, agent_id, run_id, text)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![unix_now(), source, kind, task_id, agent_id, run_id, text],
        )
        .context("failed to append event")?;
    Ok(())
}

pub fn next_task_id(existing_count: usize) -> Result<String> {
    let connection = open_db()?;
    let count: i64 = connection
        .query_row("select count(*) from tasks", [], |row| row.get(0))
        .context("failed to count tasks")?;
    Ok(format!(
        "qcd-{:04}",
        usize::try_from(count).unwrap_or(existing_count) + 1
    ))
}

#[allow(
    clippy::too_many_lines,
    reason = "schema bootstrap is kept inline so migrations remain ordered and auditable"
)]
fn open_db() -> Result<Connection> {
    let path = db_path()?;
    fs::create_dir_all(
        path.parent()
            .context("state db path has no parent directory")?,
    )?;
    let connection =
        Connection::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
    connection
        .execute_batch(
            "pragma journal_mode = wal;
             pragma foreign_keys = on;
             create table if not exists agents (
                 id text primary key,
                 track text not null,
                 pid integer not null,
                 started_at_unix integer not null,
                 command_json text not null,
                 stdout_log_path text,
                 stderr_log_path text,
                 created_at_unix integer not null
             );
             create table if not exists runs (
                 id text primary key,
                 agent_id text references agents(id),
                 kind text not null,
                 status text not null,
                 pid integer,
                 command_json text,
                 cwd text,
                 started_at_unix integer,
                 finished_at_unix integer,
                 exit_code integer,
                 metadata_json text
             );
             create table if not exists tasks (
                 id text primary key,
                 source text not null,
                 title text not null,
                 description text not null,
                 status text not null,
                 created_at_unix integer not null,
                 updated_at_unix integer not null
             );
             create table if not exists task_topics (
                 task_id text primary key references tasks(id),
                 chat_id text not null,
                 thread_id integer not null,
                 topic_name text not null,
                 source_message_id integer not null,
                 unique(chat_id, thread_id)
             );
             create table if not exists history (
                 id integer primary key autoincrement,
                 timestamp_unix integer not null,
                 source text not null,
                 role text not null,
                 text text not null,
                 task_id text references tasks(id),
                 event_id integer references events(id)
             );
             create index if not exists history_timestamp on history(timestamp_unix);
             create table if not exists events (
                 id integer primary key autoincrement,
                 timestamp_unix integer not null,
                 source text not null,
                 kind text not null,
                 task_id text references tasks(id),
                 agent_id text references agents(id),
                 run_id text references runs(id),
                 text text,
                 metadata_json text
             );
             create table if not exists terminal_metadata (
                 target text primary key,
                 name text,
                 scope text,
                 updated_at_unix integer not null
             );
             create table if not exists claims (
                 id text primary key,
                 task_id text references tasks(id),
                 owner text not null,
                 scope text not null,
                 status text not null,
                 claimed_at_unix integer not null,
                 expires_at_unix integer,
                 released_at_unix integer,
                 metadata_json text
             );
             create table if not exists budgets (
                 id text primary key,
                 subject_type text not null,
                 subject_id text not null,
                 kind text not null,
                 unit text not null,
                 limit_value real,
                 used_value real not null default 0,
                 metadata_json text
             );
             create table if not exists recipes (
                 id text primary key,
                 name text not null,
                 version text not null,
                 enabled integer not null default 1,
                 description text,
                 command_template text,
                 metadata_json text,
                 unique(name, version)
             );
             pragma user_version = 1;",
        )
        .context("failed to initialize state database")?;
    Ok(connection)
}

fn backfill_history(connection: &Connection) -> Result<()> {
    if table_count(connection, "history")? > 0 || !table_exists(connection, "messages")? {
        return Ok(());
    }
    connection
        .execute(
            "insert into history (id, timestamp_unix, source, role, text)
             select id, timestamp, source, role, text from messages order by id",
            [],
        )
        .context("failed to backfill history from messages")?;
    Ok(())
}

fn backfill_agents(connection: &Connection, legacy_path: &Path) -> Result<()> {
    if table_count(connection, "agents")? > 0 || !legacy_path.is_file() {
        return Ok(());
    }
    let log_dir = legacy_path
        .parent()
        .map_or_else(|| PathBuf::from("logs"), |path| path.join("logs"));
    for line in fs::read_to_string(legacy_path)?.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let row = parse_legacy_agent(line, &log_dir)?;
        connection.execute(
            "insert or ignore into agents
                 (id, track, pid, started_at_unix, command_json, stdout_log_path, stderr_log_path, created_at_unix)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?4)",
            params![
                row.id,
                row.track,
                row.pid,
                row.started_at,
                serde_json::to_string(&row.command)?,
                row.stdout_log_path.as_ref().map(|path| path.display().to_string()),
                row.stderr_log_path.as_ref().map(|path| path.display().to_string()),
            ],
        )?;
    }
    Ok(())
}

fn backfill_task_topics(connection: &Connection, legacy_path: &Path) -> Result<()> {
    if table_count(connection, "tasks")? > 0 || !legacy_path.is_file() {
        return Ok(());
    }
    let mut insert_task = connection.prepare(
        "insert or ignore into tasks
             (id, source, title, description, status, created_at_unix, updated_at_unix)
         values (?1, 'telegram', ?2, ?3, ?4, ?5, ?5)",
    )?;
    let mut insert_topic = connection.prepare(
        "insert or ignore into task_topics
             (task_id, chat_id, thread_id, topic_name, source_message_id)
         values (?1, ?2, ?3, ?4, ?5)",
    )?;
    for line in fs::read_to_string(legacy_path)?.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let row = parse_legacy_task_topic(line)?;
        insert_task.execute(params![
            row.id,
            row.title,
            row.description,
            row.status,
            row.created_at,
        ])?;
        insert_topic.execute(params![
            row.id,
            row.chat_id,
            row.thread_id,
            row.topic_name,
            row.source_message_id,
        ])?;
    }
    Ok(())
}

fn backfill_task_events(connection: &Connection, legacy_events_dir: &Path) -> Result<()> {
    if table_count(connection, "events")? > 0 || !legacy_events_dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(legacy_events_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let task_id = entry
            .path()
            .file_stem()
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned)
            .context("legacy task event file has no valid stem")?;
        for line in fs::read_to_string(entry.path())?.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let (timestamp, text) = parse_legacy_event(line)?;
            connection.execute(
                "insert into events (timestamp_unix, source, kind, task_id, text)
                 values (?1, 'telegram', 'task.input', ?2, ?3)",
                params![timestamp, task_id, text],
            )?;
        }
    }
    Ok(())
}

fn task_topic_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskTopicRow> {
    Ok(TaskTopicRow {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        thread_id: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        source_message_id: row.get(5)?,
        created_at: row.get(6)?,
        status: row.get(7)?,
        topic_name: row.get(8)?,
    })
}

fn parse_legacy_agent(line: &str, log_dir: &Path) -> Result<AgentRow> {
    let fields = line.split('\t').collect::<Vec<_>>();
    if fields.len() != 5 {
        bail!("invalid agent registry line: {line}");
    }
    let id = unescape_field(fields[0]);
    Ok(AgentRow {
        track: unescape_field(fields[1]),
        pid: fields[2].parse()?,
        started_at: fields[3].parse()?,
        command: unescape_field(fields[4])
            .split('\u{1f}')
            .map(ToString::to_string)
            .collect(),
        stdout_log_path: Some(log_dir.join(format!("{id}.out.log"))),
        stderr_log_path: Some(log_dir.join(format!("{id}.err.log"))),
        id,
    })
}

fn parse_legacy_task_topic(line: &str) -> Result<TaskTopicRow> {
    let fields = line.split('\t').collect::<Vec<_>>();
    if fields.len() != 8 {
        bail!("invalid telegram task registry line: {line}");
    }
    let id = unescape_field(fields[0]);
    let title = unescape_field(fields[3]);
    Ok(TaskTopicRow {
        chat_id: unescape_field(fields[1]),
        thread_id: fields[2].parse()?,
        description: unescape_field(fields[4]),
        source_message_id: fields[5].parse()?,
        created_at: fields[6].parse()?,
        status: unescape_field(fields[7]),
        topic_name: format!("{id} {title}"),
        id,
        title,
    })
}

fn parse_legacy_event(line: &str) -> Result<(u64, String)> {
    let (timestamp, text) = line
        .split_once('\t')
        .context("invalid legacy task event line")?;
    Ok((timestamp.parse()?, unescape_field(text)))
}

fn table_count(connection: &Connection, table: &str) -> Result<i64> {
    let sql = format!("select count(*) from {table}");
    connection
        .query_row(&sql, [], |row| row.get(0))
        .with_context(|| format!("failed to count {table}"))
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool> {
    connection
        .query_row(
            "select 1 from sqlite_master where type = 'table' and name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .context("failed to inspect sqlite schema")
}

fn db_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("qcold.sqlite3"))
}

pub fn state_dir() -> Result<PathBuf> {
    if let Ok(path) = env::var("QCOLD_STATE_DIR") {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    let home = env::var("HOME").context("HOME is required when QCOLD_STATE_DIR is unset")?;
    Ok(PathBuf::from(home).join(".local/state/qcold"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn unescape_field(value: &str) -> String {
    value.replace("\\t", "\t").replace("\\\\", "\\")
}
