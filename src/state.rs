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
             (id, status, selected_agent_command, selected_repo_root, selected_repo_name,
              track, current_index, stop_requested, message, created_at_unix, updated_at_unix)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            run.id,
            run.status,
            run.selected_agent_command,
            run.selected_repo_root,
            run.selected_repo_name,
            run.track,
            run.current_index,
            if run.stop_requested { 1_i64 } else { 0_i64 },
            run.message,
            run.created_at,
            run.updated_at,
        ],
    )
    .context("failed to insert web queue run")?;
    for item in items {
        tx.execute(
            "insert into web_queue_items
                 (id, run_id, position, prompt, slug, repo_root, repo_name, agent_command,
                  agent_id, status, message, attempts, next_attempt_at_unix, started_at_unix,
                  updated_at_unix)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                item.id,
                item.run_id,
                item.position,
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

pub fn load_web_queue() -> Result<(Option<QueueRunRow>, Vec<QueueItemRow>)> {
    let connection = open_db()?;
    let run = connection
        .query_row(
            "select id, status, selected_agent_command, selected_repo_root, selected_repo_name,
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
                    updated_at_unix
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
            "select id, status, selected_agent_command, selected_repo_root, selected_repo_name,
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
                    updated_at_unix
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
    Ok(requested.map_or(true, |value| value != 0))
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
    let mut connection = open_db()?;
    let tx = connection
        .transaction()
        .context("failed to start web queue item delete transaction")?;
    let item = tx
        .query_row(
            "select id, run_id, position, prompt, slug, repo_root, repo_name, agent_command,
                    agent_id, status, message, attempts, next_attempt_at_unix, started_at_unix,
                    updated_at_unix
             from web_queue_items
             where run_id = ?1 and id = ?2",
            params![run_id, item_id],
            queue_item_from_row,
        )
        .optional()
        .context("failed to query web queue item")?
        .with_context(|| format!("unknown queue item: {item_id}"))?;
    tx.execute(
        "delete from web_queue_items where run_id = ?1 and id = ?2",
        params![run_id, item_id],
    )
    .context("failed to delete web queue item")?;
    let remaining = tx
        .query_row(
            "select count(*) from web_queue_items where run_id = ?1",
            [run_id],
            |row| row.get::<_, i64>(0),
        )
        .context("failed to count remaining web queue items")?;
    if remaining == 0 {
        tx.execute("delete from web_queue_runs where id = ?1", [run_id])
            .context("failed to delete empty web queue run")?;
    }
    tx.commit().context("failed to commit web queue item delete")?;
    Ok(item)
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
    let repo_root = record
        .repo_root
        .clone()
        .or_else(|| existing.as_ref().and_then(|row| row.repo_root.clone()));
    let sequence = match (
        record.sequence,
        existing.as_ref().and_then(|row| row.sequence),
        repo_root.as_deref(),
    ) {
        (Some(sequence), _, _) | (_, Some(sequence), _) => Some(sequence),
        (None, None, Some(repo_root)) if !repo_root.trim().is_empty() => {
            Some(allocate_task_sequence(&tx, repo_root)?)
        }
        _ => None,
    };
    let created_at = existing
        .as_ref()
        .map(|row| row.created_at)
        .unwrap_or(record.created_at);
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
                 sequence = coalesce(tasks.sequence, excluded.sequence)",
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
                 cwd text,
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
                 updated_at_unix integer not null,
                 repo_root text,
                 cwd text,
                 agent_id text,
                 metadata_json text,
                 sequence integer
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
             create table if not exists web_queue_runs (
                 id text primary key,
                 status text not null,
                 selected_agent_command text not null,
                 selected_repo_root text,
                 selected_repo_name text,
                 track text not null,
                 current_index integer not null,
                 stop_requested integer not null default 0,
                 message text not null,
                 created_at_unix integer not null,
                 updated_at_unix integer not null
             );
             create table if not exists web_queue_items (
                 id text primary key,
                 run_id text not null references web_queue_runs(id) on delete cascade,
                 position integer not null,
                 prompt text not null,
                 slug text not null,
                 repo_root text,
                 repo_name text,
                 agent_command text not null,
                 agent_id text,
                 status text not null,
                 message text not null,
                 attempts integer not null default 0,
                 next_attempt_at_unix integer,
                 started_at_unix integer not null,
                 updated_at_unix integer not null,
                 unique(run_id, position),
                 unique(run_id, slug)
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
    migrate_state_schema(&connection)?;
    Ok(connection)
}

fn migrate_state_schema(connection: &Connection) -> Result<()> {
    ensure_column(connection, "tasks", "repo_root", "text")?;
    ensure_column(connection, "tasks", "cwd", "text")?;
    ensure_column(connection, "tasks", "agent_id", "text")?;
    ensure_column(connection, "tasks", "metadata_json", "text")?;
    ensure_column(connection, "tasks", "sequence", "integer")?;
    ensure_column(connection, "agents", "cwd", "text")?;
    ensure_web_queue_schema(connection)?;
    connection
        .execute(
            "create unique index if not exists tasks_repo_sequence
             on tasks(repo_root, sequence)
             where repo_root is not null and sequence is not null",
            [],
        )
        .context("failed to create task sequence index")?;
    backfill_task_sequences(connection)?;
    Ok(())
}

fn ensure_web_queue_schema(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            "create table if not exists web_queue_runs (
                 id text primary key,
                 status text not null,
                 selected_agent_command text not null,
                 selected_repo_root text,
                 selected_repo_name text,
                 track text not null,
                 current_index integer not null,
                 stop_requested integer not null default 0,
                 message text not null,
                 created_at_unix integer not null,
                 updated_at_unix integer not null
             );
             create table if not exists web_queue_items (
                 id text primary key,
                 run_id text not null references web_queue_runs(id) on delete cascade,
                 position integer not null,
                 prompt text not null,
                 slug text not null,
                 repo_root text,
                 repo_name text,
                 agent_command text not null,
                 agent_id text,
                 status text not null,
                 message text not null,
                 attempts integer not null default 0,
                 next_attempt_at_unix integer,
                 started_at_unix integer not null,
                 updated_at_unix integer not null,
                 unique(run_id, position),
                 unique(run_id, slug)
             );",
        )
        .context("failed to initialize web queue tables")?;
    Ok(())
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    if table_has_column(connection, table, column)? {
        return Ok(());
    }
    let sql = format!("alter table {table} add column {column} {definition}");
    connection
        .execute(&sql, [])
        .with_context(|| format!("failed to add {table}.{column}"))?;
    Ok(())
}

fn allocate_task_sequence(connection: &Connection, repo_root: &str) -> Result<u64> {
    let next: i64 = connection
        .query_row(
            "select coalesce(max(sequence), 0) + 1 from tasks where repo_root = ?1",
            [repo_root],
            |row| row.get(0),
        )
        .context("failed to allocate task sequence")?;
    u64::try_from(next).context("task sequence overflow")
}

fn backfill_task_sequences(connection: &Connection) -> Result<()> {
    let mut statement = connection
        .prepare(
            "select id, repo_root
             from tasks
             where repo_root is not null and trim(repo_root) != '' and sequence is null
             order by repo_root, created_at_unix, id",
        )
        .context("failed to prepare task sequence backfill")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("failed to query task sequence backfill")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode task sequence backfill rows")?;
    drop(statement);

    for (id, repo_root) in rows {
        let sequence = allocate_task_sequence(connection, &repo_root)?;
        connection
            .execute(
                "update tasks set sequence = ?1 where id = ?2 and sequence is null",
                params![sequence, id],
            )
            .with_context(|| format!("failed to backfill task sequence for {id}"))?;
    }
    Ok(())
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
                 (id, track, pid, started_at_unix, command_json, cwd, stdout_log_path, stderr_log_path, created_at_unix)
             values (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?4)",
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

fn task_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecordRow> {
    Ok(TaskRecordRow {
        id: row.get(0)?,
        source: row.get(1)?,
        sequence: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        status: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        repo_root: row.get(8)?,
        cwd: row.get(9)?,
        agent_id: row.get(10)?,
        metadata_json: row.get(11)?,
    })
}

fn queue_run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueRunRow> {
    Ok(QueueRunRow {
        id: row.get(0)?,
        status: row.get(1)?,
        selected_agent_command: row.get(2)?,
        selected_repo_root: row.get(3)?,
        selected_repo_name: row.get(4)?,
        track: row.get(5)?,
        current_index: row.get(6)?,
        stop_requested: row.get::<_, i64>(7)? != 0,
        message: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn queue_item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueItemRow> {
    Ok(QueueItemRow {
        id: row.get(0)?,
        run_id: row.get(1)?,
        position: row.get(2)?,
        prompt: row.get(3)?,
        slug: row.get(4)?,
        repo_root: row.get(5)?,
        repo_name: row.get(6)?,
        agent_command: row.get(7)?,
        agent_id: row.get(8)?,
        status: row.get(9)?,
        message: row.get(10)?,
        attempts: row.get(11)?,
        next_attempt_at: row.get(12)?,
        started_at: row.get(13)?,
        updated_at: row.get(14)?,
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
        cwd: None,
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

fn table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut statement = connection
        .prepare(&format!("pragma table_info({table})"))
        .with_context(|| format!("failed to inspect {table} columns"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .with_context(|| format!("failed to query {table} columns"))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
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
