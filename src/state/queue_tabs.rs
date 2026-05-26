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

pub fn assign_web_queue_run_to_tab(tab_id: &str, run_id: &str) -> Result<()> {
    let connection = open_db()?;
    ensure_default_web_queue_tab(&connection)?;
    let updated = connection
        .execute(
            "update web_queue_tabs set run_id = ?2, updated_at_unix = ?3 where id = ?1",
            params![tab_id, run_id, unix_now()],
        )
        .context("failed to assign queue run to tab")?;
    if updated == 0 {
        bail!("unknown queue tab: {tab_id}");
    }
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
                    remote_launcher, agent_id, status, message, attempts, next_attempt_at_unix,
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
