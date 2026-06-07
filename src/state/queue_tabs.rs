use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::db::{
    active_fallback_queue_tab_id, ensure_default_web_queue_tab, open_db, queue_item_from_row,
    queue_tab_from_row, remove_web_queue_dependency_references, unix_now,
};
use super::{QueueItemRow, QueueTabRow};

pub fn load_web_queue_tabs() -> Result<Vec<QueueTabRow>> {
    let connection = open_db()?;
    ensure_default_web_queue_tab(&connection)?;
    let mut statement = connection
        .prepare(
            "select id, label, run_id, is_default, active, created_at_unix, updated_at_unix
             from web_queue_tabs
             order by is_default desc, created_at_unix, id",
        )
        .context("failed to prepare web queue tab query")?;
    let rows = statement
        .query_map([], queue_tab_from_row)
        .context("failed to query web queue tabs")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode web queue tabs")?;
    Ok(rows)
}

pub fn load_web_queue_tab(tab_id: &str) -> Result<Option<QueueTabRow>> {
    let connection = open_db()?;
    ensure_default_web_queue_tab(&connection)?;
    connection
        .query_row(
            "select id, label, run_id, is_default, active, created_at_unix, updated_at_unix
             from web_queue_tabs
             where id = ?1",
            [tab_id],
            queue_tab_from_row,
        )
        .optional()
        .context("failed to query web queue tab")
}

pub(super) fn deduplicate_web_queue_tab_runs(connection: &Connection) -> Result<()> {
    let mut statement = connection
        .prepare(
            "select id, run_id
             from web_queue_tabs
             where run_id is not null
             order by run_id, is_default desc, created_at_unix, id",
        )
        .context("failed to prepare duplicate web queue tab repair query")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("failed to query duplicate web queue tab repair rows")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode duplicate web queue tab repair rows")?;
    let mut seen = HashSet::new();
    let duplicate_tab_ids = rows
        .into_iter()
        .filter_map(|(tab_id, run_id)| {
            if seen.insert(run_id) {
                None
            } else {
                Some(tab_id)
            }
        })
        .collect::<Vec<_>>();
    let now = unix_now();
    for tab_id in duplicate_tab_ids {
        connection
            .execute(
                "update web_queue_tabs
                 set run_id = null, updated_at_unix = ?2
                 where id = ?1",
                params![tab_id, now],
            )
            .context("failed to repair duplicate web queue tab run reference")?;
    }
    Ok(())
}

#[cfg(test)]
pub fn create_web_queue_tab(tab_id: &str, label: &str) -> Result<QueueTabRow> {
    let connection = open_db()?;
    ensure_default_web_queue_tab(&connection)?;
    let now = unix_now();
    connection
        .execute(
            "insert into web_queue_tabs
                 (id, label, run_id, is_default, active, created_at_unix, updated_at_unix)
             values (?1, ?2, null, 0, 0, ?3, ?3)",
            params![tab_id, label, now],
        )
        .context("failed to create web queue tab")?;
    load_web_queue_tab(tab_id)?.with_context(|| format!("missing created queue tab: {tab_id}"))
}

pub fn create_and_activate_web_queue_tab(tab_id: &str, label: &str) -> Result<QueueTabRow> {
    let mut connection = open_db()?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to start web queue tab create transaction")?;
    ensure_default_web_queue_tab(&tx)?;
    let now = unix_now();
    tx.execute(
        "insert into web_queue_tabs
             (id, label, run_id, is_default, active, created_at_unix, updated_at_unix)
         values (?1, ?2, null, 0, 0, ?3, ?3)",
        params![tab_id, label, now],
    )
    .context("failed to create web queue tab")?;
    tx.execute("update web_queue_tabs set active = 0", [])
        .context("failed to clear active queue tab")?;
    tx.execute(
        "update web_queue_tabs set active = 1, updated_at_unix = ?2 where id = ?1",
        params![tab_id, unix_now()],
    )
    .context("failed to activate web queue tab")?;
    let tab = tx
        .query_row(
            "select id, label, run_id, is_default, active, created_at_unix, updated_at_unix
             from web_queue_tabs
             where id = ?1",
            [tab_id],
            queue_tab_from_row,
        )
        .context("failed to load created web queue tab")?;
    tx.commit()
        .context("failed to commit web queue tab create")?;
    Ok(tab)
}

pub fn activate_web_queue_tab(tab_id: &str) -> Result<()> {
    let mut connection = open_db()?;
    let tx = connection
        .transaction()
        .context("failed to start web queue tab activation transaction")?;
    ensure_default_web_queue_tab(&tx)?;
    let exists = tx
        .query_row(
            "select 1 from web_queue_tabs where id = ?1",
            [tab_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to query web queue tab")?
        .is_some();
    if !exists {
        bail!("unknown queue tab: {tab_id}");
    }
    tx.execute("update web_queue_tabs set active = 0", [])
        .context("failed to clear active queue tab")?;
    tx.execute(
        "update web_queue_tabs set active = 1, updated_at_unix = ?2 where id = ?1",
        params![tab_id, unix_now()],
    )
    .context("failed to activate web queue tab")?;
    tx.commit()
        .context("failed to commit web queue tab activation")?;
    Ok(())
}

pub(super) fn web_queue_tab_run_id(
    connection: &Connection,
    tab_id: &str,
) -> Result<Option<String>> {
    ensure_default_web_queue_tab(connection)?;
    connection
        .query_row(
            "select run_id from web_queue_tabs where id = ?1",
            [tab_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .context("failed to query web queue tab run")?
        .with_context(|| format!("unknown queue tab: {tab_id}"))
}

pub(super) fn assign_web_queue_run_to_tab_in_connection(
    connection: &Connection,
    tab_id: &str,
    run_id: &str,
) -> Result<()> {
    ensure_default_web_queue_tab(connection)?;
    let updated = connection
        .execute(
            "update web_queue_tabs set run_id = ?2, updated_at_unix = ?3 where id = ?1",
            params![tab_id, run_id, unix_now()],
        )
        .context("failed to assign queue run to tab")?;
    if updated == 0 {
        bail!("unknown queue tab: {tab_id}");
    }
    connection
        .execute(
            "update web_queue_tabs
             set run_id = null, updated_at_unix = ?3
             where id != ?1 and run_id = ?2",
            params![tab_id, run_id, unix_now()],
        )
        .context("failed to clear duplicate queue run from other tabs")?;
    Ok(())
}

pub fn delete_web_queue_tab(tab_id: &str) -> Result<()> {
    let mut connection = open_db()?;
    let tx = connection
        .transaction()
        .context("failed to start web queue tab delete transaction")?;
    ensure_default_web_queue_tab(&tx)?;
    let tab = tx
        .query_row(
            "select id, label, run_id, is_default, active, created_at_unix, updated_at_unix
             from web_queue_tabs
             where id = ?1",
            [tab_id],
            queue_tab_from_row,
        )
        .optional()
        .context("failed to load web queue tab")?
        .with_context(|| format!("unknown queue tab: {tab_id}"))?;
    if tab.is_default {
        bail!("cannot delete the default queue tab");
    }
    tx.execute("delete from web_queue_tabs where id = ?1", [tab_id])
        .context("failed to delete web queue tab")?;
    if tab.active {
        let fallback = active_fallback_queue_tab_id(&tx)?;
        tx.execute("update web_queue_tabs set active = 0", [])
            .context("failed to clear active queue tab")?;
        tx.execute(
            "update web_queue_tabs set active = 1, updated_at_unix = ?2 where id = ?1",
            params![fallback, unix_now()],
        )
        .context("failed to activate fallback queue tab")?;
    }
    tx.commit()
        .context("failed to commit web queue tab delete")?;
    Ok(())
}

pub fn delete_web_queue_run_items(run_id: &str) -> Result<Vec<QueueItemRow>> {
    let mut connection = open_db()?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to start web queue run delete transaction")?;
    let mut statement = tx
        .prepare(
            "select id, run_id, position, prompt, slug, repo_root, repo_name, execution_host,
                    agent_command, task_class, remote_launcher, remote_agent_local_proxy, remote_agent_remote_proxy,
                    agent_id, status, message, attempts, recovery_attempts, next_attempt_at_unix,
                    started_at_unix, updated_at_unix, depends_on_json
             from web_queue_items
             where run_id = ?1
             order by position, id",
        )
        .context("failed to prepare web queue run item delete query")?;
    let items = statement
        .query_map([run_id], queue_item_from_row)
        .context("failed to query web queue run items for delete")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode web queue run items for delete")?;
    drop(statement);
    tx.execute("delete from web_queue_items where run_id = ?1", [run_id])
        .context("failed to delete web queue run items")?;
    tx.execute("delete from web_queue_runs where id = ?1", [run_id])
        .context("failed to delete web queue run")?;
    tx.execute(
        "update web_queue_tabs set run_id = null, updated_at_unix = ?2 where run_id = ?1",
        params![run_id, unix_now()],
    )
    .context("failed to detach deleted web queue run from tabs")?;
    tx.commit()
        .context("failed to commit web queue run delete")?;
    Ok(items)
}

#[cfg(test)]
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
            "select id, run_id, position, prompt, slug, repo_root, repo_name, execution_host,
                    agent_command, task_class, remote_launcher, remote_agent_local_proxy, remote_agent_remote_proxy,
                    agent_id, status, message, attempts, recovery_attempts, next_attempt_at_unix,
                    started_at_unix, updated_at_unix, depends_on_json
             from web_queue_items
             where run_id = ?1 and id = ?2",
            params![run_id, item_id],
            queue_item_from_row,
        )
        .optional()
        .context("failed to query web queue item")?;
    let Some(item) = item else {
        delete_web_queue_run_if_empty(&tx, run_id)?;
        tx.commit()
            .context("failed to commit web queue item delete")?;
        return Ok(None);
    };
    tx.execute(
        "delete from web_queue_items where run_id = ?1 and id = ?2",
        params![run_id, item_id],
    )
    .context("failed to delete web queue item")?;
    remove_web_queue_dependency_references(&tx, run_id, item_id)?;
    delete_web_queue_run_if_empty(&tx, run_id)?;
    tx.commit()
        .context("failed to commit web queue item delete")?;
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

pub(super) fn delete_unreferenced_web_queue_run(
    connection: &Connection,
    run_id: &str,
) -> Result<()> {
    let references = connection
        .query_row(
            "select count(*) from web_queue_tabs where run_id = ?1",
            [run_id],
            |row| row.get::<_, i64>(0),
        )
        .context("failed to count web queue tab references")?;
    if references == 0 {
        delete_web_queue_run(connection, run_id)?;
    }
    Ok(())
}

pub(super) fn delete_unreferenced_web_queue_runs(connection: &Connection) -> Result<()> {
    connection
        .execute(
            "delete from web_queue_items
             where not exists (
                 select 1 from web_queue_tabs where web_queue_tabs.run_id = web_queue_items.run_id
             )",
            [],
        )
        .context("failed to delete orphaned web queue items")?;
    connection
        .execute(
            "delete from web_queue_runs
             where not exists (
                 select 1 from web_queue_tabs where web_queue_tabs.run_id = web_queue_runs.id
             )",
            [],
        )
        .context("failed to delete orphaned web queue runs")?;
    Ok(())
}

fn delete_web_queue_run(connection: &Connection, run_id: &str) -> Result<()> {
    connection
        .execute("delete from web_queue_items where run_id = ?1", [run_id])
        .context("failed to delete web queue items")?;
    connection
        .execute("delete from web_queue_runs where id = ?1", [run_id])
        .context("failed to delete web queue run")?;
    Ok(())
}
