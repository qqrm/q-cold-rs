use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

const DEFAULT_SQLITE_BUSY_TIMEOUT_MS: u64 = 30_000;

#[derive(Clone, Debug)]
pub struct AgentRow {
    pub id: String,
    pub track: String,
    pub pid: u32,
    pub started_at: u64,
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
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
pub struct TaskRecordRow {
    pub id: String,
    pub source: String,
    pub sequence: Option<u64>,
    pub title: String,
    pub description: String,
    pub status: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub repo_root: Option<String>,
    pub cwd: Option<String>,
    pub agent_id: Option<String>,
    pub metadata_json: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TerminalMetadataRow {
    pub target: String,
    pub name: Option<String>,
    pub scope: Option<String>,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct QueueRunRow {
    pub id: String,
    pub status: String,
    pub execution_mode: String,
    pub selected_agent_command: String,
    pub selected_repo_root: Option<String>,
    pub selected_repo_name: Option<String>,
    pub track: String,
    pub current_index: i64,
    pub stop_requested: bool,
    pub message: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct QueueItemRow {
    pub id: String,
    pub run_id: String,
    pub position: i64,
    pub depends_on: Vec<String>,
    pub prompt: String,
    pub slug: String,
    pub repo_root: Option<String>,
    pub repo_name: Option<String>,
    pub agent_command: String,
    pub agent_id: Option<String>,
    pub status: String,
    pub message: String,
    pub attempts: i64,
    pub next_attempt_at: Option<u64>,
    pub started_at: u64,
    pub updated_at: u64,
}

pub fn load_agents(legacy_path: &Path) -> Result<Vec<AgentRow>> {
    let connection = open_db()?;
    backfill_agents(&connection, legacy_path)?;
    let mut statement = connection
        .prepare(
            "select id, track, pid, started_at_unix, command_json, cwd, stdout_log_path, stderr_log_path
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
                cwd: row.get::<_, Option<String>>(5)?.map(PathBuf::from),
                stdout_log_path: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
                stderr_log_path: row.get::<_, Option<String>>(7)?.map(PathBuf::from),
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
                 (id, track, pid, started_at_unix, command_json, cwd, stdout_log_path, stderr_log_path, created_at_unix)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?4)",
            params![
                agent.id,
                agent.track,
                agent.pid,
                agent.started_at,
                serde_json::to_string(&agent.command)?,
                agent.cwd.as_ref().map(|path| path.display().to_string()),
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

pub fn replace_web_queue(run: &QueueRunRow, items: &[QueueItemRow]) -> Result<()> {
    let mut connection = open_db()?;
    let tx = connection
        .transaction()
        .context("failed to start web queue transaction")?;
    tx.execute("delete from web_queue_items", [])
        .context("failed to clear web queue items")?;
    tx.execute("delete from web_queue_runs", [])
        .context("failed to clear web queue runs")?;
    tx.execute(
        "insert into web_queue_runs
             (id, status, execution_mode, selected_agent_command, selected_repo_root, selected_repo_name,
              track, current_index, stop_requested, message, created_at_unix, updated_at_unix)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            run.id,
            run.status,
            run.execution_mode,
            run.selected_agent_command,
            run.selected_repo_root,
            run.selected_repo_name,
            run.track,
            run.current_index,
            i64::from(run.stop_requested),
            run.message,
            run.created_at,
            run.updated_at,
        ],
    )
    .context("failed to insert web queue run")?;
    for item in items {
        tx.execute(
            "insert into web_queue_items
                 (id, run_id, position, depends_on_json, prompt, slug, repo_root, repo_name, agent_command,
                  agent_id, status, message, attempts, next_attempt_at_unix, started_at_unix,
                  updated_at_unix)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                item.id,
                item.run_id,
                item.position,
                queue_depends_on_json(&item.depends_on)?,
                item.prompt,
                item.slug,
                item.repo_root,
                item.repo_name,
                item.agent_command,
                item.agent_id,
                item.status,
                item.message,
                item.attempts,
                item.next_attempt_at,
                item.started_at,
                item.updated_at,
            ],
        )
        .context("failed to insert web queue item")?;
    }
    tx.commit().context("failed to commit web queue")?;
    Ok(())
}

pub fn append_web_queue_items(run_id: &str, items: &[QueueItemRow]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let mut connection = open_db()?;
    let tx = connection
        .transaction()
        .context("failed to start web queue append transaction")?;
    let exists = tx
        .query_row(
            "select 1 from web_queue_runs where id = ?1",
            [run_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to query web queue run")?
        .is_some();
    if !exists {
        bail!("unknown queue run: {run_id}");
    }
    for item in items {
        tx.execute(
            "insert into web_queue_items
                 (id, run_id, position, depends_on_json, prompt, slug, repo_root, repo_name, agent_command,
                  agent_id, status, message, attempts, next_attempt_at_unix, started_at_unix,
                  updated_at_unix)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                item.id,
                item.run_id,
                item.position,
                queue_depends_on_json(&item.depends_on)?,
                item.prompt,
                item.slug,
                item.repo_root,
                item.repo_name,
                item.agent_command,
                item.agent_id,
                item.status,
                item.message,
                item.attempts,
                item.next_attempt_at,
                item.started_at,
                item.updated_at,
            ],
        )
        .context("failed to append web queue item")?;
    }
    tx.execute(
        "update web_queue_runs
         set message = ?2, updated_at_unix = ?3
         where id = ?1",
        params![
            run_id,
            format!("appended {} queue item(s)", items.len()),
            unix_now(),
        ],
    )
    .context("failed to update web queue run after append")?;
    tx.commit()
        .context("failed to commit web queue append transaction")?;
    Ok(())
}

pub fn update_web_queue_item_plans(run_id: &str, items: &[QueueItemRow]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let mut connection = open_db()?;
    let tx = connection
        .transaction()
        .context("failed to start web queue update transaction")?;
    let now = unix_now();
    for item in items {
        tx.execute(
            "update web_queue_items
             set position = ?3, depends_on_json = ?4, prompt = ?5, repo_root = ?6,
                 repo_name = ?7, agent_command = ?8, updated_at_unix = ?9
             where run_id = ?1 and id = ?2",
            params![
                run_id,
                item.id,
                item.position,
                queue_depends_on_json(&item.depends_on)?,
                item.prompt,
                item.repo_root,
                item.repo_name,
                item.agent_command,
                now,
            ],
        )
        .context("failed to update web queue item plan")?;
    }
    tx.execute(
        "update web_queue_runs
         set message = ?2, updated_at_unix = ?3
         where id = ?1",
        params![run_id, format!("updated {} queue item(s)", items.len()), now],
    )
    .context("failed to update web queue run after item plan update")?;
    tx.commit().context("failed to commit web queue update transaction")?;
    Ok(())
}

pub fn load_web_queue() -> Result<(Option<QueueRunRow>, Vec<QueueItemRow>)> {
    let connection = open_db()?;
    let run = connection
        .query_row(
            "select id, status, execution_mode, selected_agent_command, selected_repo_root, selected_repo_name,
                    track, current_index, stop_requested, message, created_at_unix, updated_at_unix
             from web_queue_runs
             order by updated_at_unix desc
             limit 1",
            [],
            queue_run_from_row,
        )
        .optional()
        .context("failed to query web queue run")?;
    let Some(run_row) = run else {
        return Ok((None, Vec::new()));
    };
    let mut statement = connection
        .prepare(
            "select id, run_id, position, prompt, slug, repo_root, repo_name, agent_command,
                    agent_id, status, message, attempts, next_attempt_at_unix, started_at_unix,
                    updated_at_unix, depends_on_json
             from web_queue_items
             where run_id = ?1
             order by position, id",
        )
        .context("failed to prepare web queue item query")?;
    let rows = statement
        .query_map([run_row.id.as_str()], queue_item_from_row)
        .context("failed to query web queue items")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode web queue items")?;
    Ok((Some(run_row), rows))
}

pub fn load_web_queue_run(run_id: &str) -> Result<(Option<QueueRunRow>, Vec<QueueItemRow>)> {
    let connection = open_db()?;
    let run = connection
        .query_row(
            "select id, status, execution_mode, selected_agent_command, selected_repo_root, selected_repo_name,
                    track, current_index, stop_requested, message, created_at_unix, updated_at_unix
             from web_queue_runs
             where id = ?1",
            [run_id],
            queue_run_from_row,
        )
        .optional()
        .context("failed to query web queue run")?;
    let Some(run_row) = run else {
        return Ok((None, Vec::new()));
    };
    let mut statement = connection
        .prepare(
            "select id, run_id, position, prompt, slug, repo_root, repo_name, agent_command,
                    agent_id, status, message, attempts, next_attempt_at_unix, started_at_unix,
                    updated_at_unix, depends_on_json
             from web_queue_items
             where run_id = ?1
             order by position, id",
        )
        .context("failed to prepare web queue item query")?;
    let rows = statement
        .query_map([run_row.id.as_str()], queue_item_from_row)
        .context("failed to query web queue items")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode web queue items")?;
    Ok((Some(run_row), rows))
}

pub fn update_web_queue_run(
    run_id: &str,
    status: &str,
    current_index: i64,
    message: &str,
) -> Result<()> {
    let connection = open_db()?;
    connection
        .execute(
            "update web_queue_runs
             set status = ?2, current_index = ?3, message = ?4, updated_at_unix = ?5
             where id = ?1",
            params![run_id, status, current_index, message, unix_now()],
        )
        .context("failed to update web queue run")?;
    Ok(())
}

pub fn request_web_queue_stop() -> Result<()> {
    let connection = open_db()?;
    connection
        .execute(
            "update web_queue_runs
             set stop_requested = 1, status = 'stopping', message = 'stop requested',
                 updated_at_unix = ?1
             where status in ('running', 'waiting', 'starting')",
            [unix_now()],
        )
        .context("failed to request web queue stop")?;
    Ok(())
}

pub fn continue_web_queue_run(run_id: &str) -> Result<()> {
    let connection = open_db()?;
    let updated = connection
        .execute(
            "update web_queue_runs
             set stop_requested = 0, status = 'running', message = 'continued',
                 updated_at_unix = ?2
             where id = ?1 and status = 'stopped'",
            params![run_id, unix_now()],
        )
        .context("failed to continue web queue")?;
    if updated == 0 {
        bail!("queue is not stopped: {run_id}");
    }
    Ok(())
}

pub fn web_queue_stop_requested(run_id: &str) -> Result<bool> {
    let connection = open_db()?;
    let requested = connection
        .query_row(
            "select stop_requested from web_queue_runs where id = ?1",
            [run_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to query web queue stop flag")?;
    Ok(requested != Some(0))
}

pub fn update_web_queue_item(
    run_id: &str,
    item_id: &str,
    status: &str,
    message: &str,
    agent_id: Option<&str>,
    attempts: i64,
    next_attempt_at: Option<u64>,
) -> Result<()> {
    let connection = open_db()?;
    connection
        .execute(
            "update web_queue_items
             set status = ?3, message = ?4, agent_id = coalesce(?5, agent_id),
                 attempts = ?6, next_attempt_at_unix = ?7, updated_at_unix = ?8
             where run_id = ?1 and id = ?2",
            params![
                run_id,
                item_id,
                status,
                message,
                agent_id,
                attempts,
                next_attempt_at,
                unix_now(),
            ],
        )
        .context("failed to update web queue item")?;
    Ok(())
}

pub fn set_web_queue_item_agent(run_id: &str, item_id: &str, agent_id: &str) -> Result<()> {
    let connection = open_db()?;
    connection
        .execute(
            "update web_queue_items
             set agent_id = ?3, updated_at_unix = ?4
             where run_id = ?1 and id = ?2",
            params![run_id, item_id, agent_id, unix_now()],
        )
        .context("failed to update web queue item agent")?;
    Ok(())
}

pub fn delete_web_queue_item(run_id: &str, item_id: &str) -> Result<QueueItemRow> {
    delete_web_queue_item_if_exists(run_id, item_id)?
        .with_context(|| format!("unknown queue item: {item_id}"))
}

pub fn delete_web_queue_item_if_exists(
    run_id: &str,
    item_id: &str,
) -> Result<Option<QueueItemRow>> {
    let mut connection = open_db()?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to start web queue item delete transaction")?;
    let item = tx
        .query_row(
            "select id, run_id, position, prompt, slug, repo_root, repo_name, agent_command,
                    agent_id, status, message, attempts, next_attempt_at_unix, started_at_unix,
                    updated_at_unix, depends_on_json
             from web_queue_items
             where run_id = ?1 and id = ?2",
            params![run_id, item_id],
            queue_item_from_row,
        )
        .optional()
        .context("failed to query web queue item")?;
    let Some(item) = item else {
        delete_web_queue_run_if_empty(&tx, run_id)?;
        tx.commit().context("failed to commit web queue item delete")?;
        return Ok(None);
    };
    tx.execute(
        "delete from web_queue_items where run_id = ?1 and id = ?2",
        params![run_id, item_id],
    )
    .context("failed to delete web queue item")?;
    remove_web_queue_dependency_references(&tx, run_id, item_id)?;
    delete_web_queue_run_if_empty(&tx, run_id)?;
    tx.commit().context("failed to commit web queue item delete")?;
    Ok(Some(item))
}

fn delete_web_queue_run_if_empty(connection: &Connection, run_id: &str) -> Result<()> {
    let remaining = connection
        .query_row(
            "select count(*) from web_queue_items where run_id = ?1",
            [run_id],
            |row| row.get::<_, i64>(0),
        )
        .context("failed to count remaining web queue items")?;
    if remaining == 0 {
        connection
            .execute("delete from web_queue_runs where id = ?1", [run_id])
            .context("failed to delete empty web queue run")?;
    }
    Ok(())
}

pub fn delete_empty_web_queue_run(run_id: &str) -> Result<bool> {
    let connection = open_db()?;
    let deleted = connection
        .execute(
            "delete from web_queue_runs
             where id = ?1
               and not exists (
                   select 1 from web_queue_items where run_id = ?1
               )",
            [run_id],
        )
        .context("failed to delete empty web queue run")?;
    Ok(deleted > 0)
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

pub fn upsert_task_record(record: &TaskRecordRow) -> Result<TaskRecordRow> {
    let mut connection = open_db()?;
    let tx = connection
        .transaction()
        .context("failed to start task record transaction")?;
    let existing = tx
        .query_row(
            "select id, source, sequence, title, description, status, created_at_unix, updated_at_unix,
                    repo_root, cwd, agent_id, metadata_json
             from tasks
             where id = ?1",
            [record.id.as_str()],
            task_record_from_row,
        )
        .optional()
        .context("failed to load existing task record")?;
    let existing_repo_root = existing.as_ref().and_then(|row| row.repo_root.clone());
    let repo_root = record
        .repo_root
        .clone()
        .or_else(|| existing_repo_root.clone());
    let repo_root_changed = matches!(
        (record.repo_root.as_deref(), existing_repo_root.as_deref()),
        (Some(new), Some(old)) if new != old
    );
    let sequence =
        task_record_sequence_for_upsert(&tx, record, existing.as_ref(), repo_root.as_deref(), repo_root_changed)?;
    advance_task_sequence_counter_for_record(&tx, repo_root.as_deref(), sequence)?;
    let created_at = existing
        .as_ref()
        .map_or(record.created_at, |row| row.created_at);
    let cwd = record
        .cwd
        .clone()
        .or_else(|| existing.as_ref().and_then(|row| row.cwd.clone()));
    let agent_id = record
        .agent_id
        .clone()
        .or_else(|| existing.as_ref().and_then(|row| row.agent_id.clone()));
    let metadata_json = record
        .metadata_json
        .clone()
        .or_else(|| existing.as_ref().and_then(|row| row.metadata_json.clone()));

    tx.execute(
            "insert into tasks
                 (id, source, title, description, status, created_at_unix, updated_at_unix,
                  repo_root, cwd, agent_id, metadata_json, sequence)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             on conflict(id) do update set
                 source = excluded.source,
                 title = excluded.title,
                 description = excluded.description,
                 status = excluded.status,
                 updated_at_unix = excluded.updated_at_unix,
                 repo_root = coalesce(excluded.repo_root, tasks.repo_root),
                 cwd = coalesce(excluded.cwd, tasks.cwd),
                 agent_id = coalesce(excluded.agent_id, tasks.agent_id),
                 metadata_json = coalesce(excluded.metadata_json, tasks.metadata_json),
                 sequence = excluded.sequence",
            params![
                record.id,
                record.source,
                record.title,
                record.description,
                record.status,
                created_at,
                record.updated_at,
                repo_root,
                cwd,
                agent_id,
                metadata_json,
                sequence,
            ],
        )
        .context("failed to upsert task record")?;
    let stored = tx
        .query_row(
            "select id, source, sequence, title, description, status, created_at_unix, updated_at_unix,
                    repo_root, cwd, agent_id, metadata_json
             from tasks
             where id = ?1",
            [record.id.as_str()],
            task_record_from_row,
        )
        .context("failed to reload task record")?;
    tx.commit().context("failed to commit task record")?;
    Ok(stored)
}

fn advance_task_sequence_counter_for_record(
    connection: &Connection,
    repo_root: Option<&str>,
    sequence: Option<u64>,
) -> Result<()> {
    let (Some(repo_root), Some(sequence)) = (repo_root, sequence) else {
        return Ok(());
    };
    if !repo_root.trim().is_empty() {
        advance_task_sequence_counter(connection, repo_root, sequence)?;
    }
    Ok(())
}

fn source_uses_task_sequence(source: &str) -> bool {
    !matches!(source, "agent" | "codex-session")
}

pub fn load_task_records(status: Option<&str>, limit: usize) -> Result<Vec<TaskRecordRow>> {
    let connection = open_db()?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut records = Vec::new();
    if let Some(status) = status {
        let mut statement = connection
            .prepare(
                "select id, source, sequence, title, description, status, created_at_unix, updated_at_unix,
                        repo_root, cwd, agent_id, metadata_json
                 from tasks
                 where status = ?1
                 order by updated_at_unix desc, id
                 limit ?2",
            )
            .context("failed to prepare task record query")?;
        let rows = statement
            .query_map(params![status, limit], task_record_from_row)
            .context("failed to query task records")?;
        for row in rows {
            records.push(row.context("failed to decode task record")?);
        }
    } else {
        let mut statement = connection
            .prepare(
                "select id, source, sequence, title, description, status, created_at_unix, updated_at_unix,
                        repo_root, cwd, agent_id, metadata_json
                 from tasks
                 order by updated_at_unix desc, id
                 limit ?1",
            )
            .context("failed to prepare task record query")?;
        let rows = statement
            .query_map([limit], task_record_from_row)
            .context("failed to query task records")?;
        for row in rows {
            records.push(row.context("failed to decode task record")?);
        }
    }
    Ok(records)
}

pub fn load_task_records_for_repo(
    repo_root: &str,
    status: Option<&str>,
    limit: usize,
) -> Result<Vec<TaskRecordRow>> {
    let connection = open_db()?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut records = Vec::new();
    if let Some(status) = status {
        let mut statement = connection
            .prepare(
                "select id, source, sequence, title, description, status, created_at_unix, updated_at_unix,
                        repo_root, cwd, agent_id, metadata_json
                 from tasks
                 where repo_root = ?1 and status = ?2
                 order by updated_at_unix desc, id
                 limit ?3",
            )
            .context("failed to prepare repo task record query")?;
        let rows = statement
            .query_map(params![repo_root, status, limit], task_record_from_row)
            .context("failed to query repo task records")?;
        for row in rows {
            records.push(row.context("failed to decode repo task record")?);
        }
    } else {
        let mut statement = connection
            .prepare(
                "select id, source, sequence, title, description, status, created_at_unix, updated_at_unix,
                        repo_root, cwd, agent_id, metadata_json
                 from tasks
                 where repo_root = ?1
                 order by updated_at_unix desc, id
                 limit ?2",
            )
            .context("failed to prepare repo task record query")?;
        let rows = statement
            .query_map(params![repo_root, limit], task_record_from_row)
            .context("failed to query repo task records")?;
        for row in rows {
            records.push(row.context("failed to decode repo task record")?);
        }
    }
    Ok(records)
}

pub fn get_task_record(id: &str) -> Result<Option<TaskRecordRow>> {
    let connection = open_db()?;
    connection
        .query_row(
            "select id, source, sequence, title, description, status, created_at_unix, updated_at_unix,
                    repo_root, cwd, agent_id, metadata_json
             from tasks
             where id = ?1",
            [id],
            task_record_from_row,
        )
        .optional()
        .context("failed to load task record")
}

pub fn update_task_record(
    id: &str,
    title: Option<&str>,
    description: Option<&str>,
    status: Option<&str>,
) -> Result<()> {
    let mut record = get_task_record(id)?.with_context(|| format!("unknown task record: {id}"))?;
    if let Some(title) = title {
        record.title = title.to_string();
    }
    if let Some(description) = description {
        record.description = description.to_string();
    }
    if let Some(status) = status {
        record.status = status.to_string();
    }
    record.updated_at = unix_now();
    upsert_task_record(&record).map(|_| ())
}

pub fn delete_task_record(id: &str) -> Result<()> {
    let mut connection = open_db()?;
    let tx = connection
        .transaction()
        .context("failed to start task delete transaction")?;
    tx.execute("delete from task_topics where task_id = ?1", [id])
        .context("failed to delete task topic")?;
    tx.execute("update history set task_id = null where task_id = ?1", [id])
        .context("failed to detach task history")?;
    tx.execute("update events set task_id = null where task_id = ?1", [id])
        .context("failed to detach task events")?;
    tx.execute("update claims set task_id = null where task_id = ?1", [id])
        .context("failed to detach task claims")?;
    let deleted = tx
        .execute("delete from tasks where id = ?1", [id])
        .context("failed to delete task record")?;
    if deleted == 0 {
        bail!("unknown task record: {id}");
    }
    tx.commit().context("failed to commit task delete")?;
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
    clippy::too_many_arguments,
    reason = "task records are assembled from command/API boundaries with explicit fields"
)]
pub fn new_task_record(
    id: String,
    source: String,
    title: String,
    description: String,
    status: String,
    repo_root: Option<String>,
    cwd: Option<String>,
    agent_id: Option<String>,
    metadata_json: Option<String>,
) -> TaskRecordRow {
    let now = unix_now();
    TaskRecordRow {
        id,
        source,
        sequence: None,
        title,
        description,
        status,
        created_at: now,
        updated_at: now,
        repo_root,
        cwd,
        agent_id,
        metadata_json,
    }
}


include!("state/db.rs");
