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
        .busy_timeout(Duration::from_secs(5))
        .context("failed to set state database busy timeout")?;
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
             create table if not exists task_sequence_counters (
                 repo_root text primary key,
                 next_sequence integer not null
             );
             create table if not exists task_topics (
                 task_id text primary key references tasks(id),
                 chat_id text not null,
                 thread_id integer not null,
                 topic_name text not null,
                 source_message_id integer not null,
                 unique(chat_id, thread_id)
             );
             create table if not exists schema_migrations (
                 name text primary key,
                 applied_at_unix integer not null
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
                 execution_mode text not null default 'sequence',
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
                 depends_on_json text not null default '[]',
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
    ensure_column(
        connection,
        "web_queue_runs",
        "execution_mode",
        "text not null default 'sequence'",
    )?;
    ensure_column(
        connection,
        "web_queue_items",
        "depends_on_json",
        "text not null default '[]'",
    )?;
    ensure_schema_migrations(connection)?;
    connection
        .execute(
            "create table if not exists task_sequence_counters (
                 repo_root text primary key,
                 next_sequence integer not null
             )",
            [],
        )
        .context("failed to create task sequence counters table")?;
    repair_legacy_task_sequence_pollution_once(connection)?;
    scrub_non_task_sequences(connection)?;
    connection
        .execute(
            "create unique index if not exists tasks_repo_sequence
             on tasks(repo_root, sequence)
             where repo_root is not null and sequence is not null",
            [],
        )
        .context("failed to create task sequence index")?;
    seed_task_sequence_counters(connection)?;
    backfill_task_sequences(connection)?;
    Ok(())
}

fn ensure_web_queue_schema(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            "create table if not exists web_queue_runs (
                 id text primary key,
                 status text not null,
                 execution_mode text not null default 'sequence',
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
                 depends_on_json text not null default '[]',
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

fn ensure_schema_migrations(connection: &Connection) -> Result<()> {
    connection
        .execute(
            "create table if not exists schema_migrations (
                 name text primary key,
                 applied_at_unix integer not null
             )",
            [],
        )
        .context("failed to create schema migrations table")?;
    Ok(())
}

fn allocate_task_sequence(connection: &Connection, repo_root: &str) -> Result<u64> {
    let initial_next = max_task_sequence(connection, repo_root)?.saturating_add(1);
    connection
        .execute(
            "insert into task_sequence_counters (repo_root, next_sequence)
             values (?1, ?2)
             on conflict(repo_root) do nothing",
            params![repo_root, i64::try_from(initial_next).unwrap_or(i64::MAX)],
        )
        .context("failed to initialize task sequence counter")?;
    let next: i64 = connection
        .query_row(
            "update task_sequence_counters
             set next_sequence = next_sequence + 1
             where repo_root = ?1
             returning next_sequence - 1",
            params![repo_root],
            |row| row.get(0),
        )
        .context("failed to allocate task sequence")?;
    u64::try_from(next).context("task sequence overflow")
}

fn task_record_sequence_for_upsert(
    connection: &Connection,
    record: &TaskRecordRow,
    existing: Option<&TaskRecordRow>,
    repo_root: Option<&str>,
    repo_root_changed: bool,
) -> Result<Option<u64>> {
    if !source_uses_task_sequence(&record.source) {
        return Ok(None);
    }
    let existing_sequence =
        existing.and_then(|row| (!repo_root_changed).then_some(row.sequence).flatten());
    match (record.sequence, existing_sequence, repo_root) {
        (Some(sequence), _, _) | (_, Some(sequence), _) => Ok(Some(sequence)),
        (None, None, Some(repo_root)) if !repo_root.trim().is_empty() => {
            allocate_task_sequence(connection, repo_root).map(Some)
        }
        _ => Ok(None),
    }
}

fn advance_task_sequence_counter(
    connection: &Connection,
    repo_root: &str,
    sequence: u64,
) -> Result<()> {
    let next = i64::try_from(sequence.saturating_add(1)).unwrap_or(i64::MAX);
    connection
        .execute(
            "insert into task_sequence_counters (repo_root, next_sequence)
             values (?1, ?2)
             on conflict(repo_root) do update set
                 next_sequence = max(task_sequence_counters.next_sequence, excluded.next_sequence)",
            params![repo_root, next],
        )
        .context("failed to advance task sequence counter")?;
    Ok(())
}

fn seed_task_sequence_counters(connection: &Connection) -> Result<()> {
    let mut statement = connection
        .prepare(
            "select repo_root, coalesce(max(sequence), 0) + 1
             from tasks
             where repo_root is not null and trim(repo_root) != ''
               and source not in ('agent', 'codex-session')
             group by repo_root",
        )
        .context("failed to prepare task sequence counter seed")?;
    let rows = statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
        .context("failed to query task sequence counter seed")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode task sequence counter seed")?;
    drop(statement);

    for (repo_root, next_sequence) in rows {
        connection
            .execute(
                "insert into task_sequence_counters (repo_root, next_sequence)
                 values (?1, ?2)
                 on conflict(repo_root) do update set
                     next_sequence = max(task_sequence_counters.next_sequence, excluded.next_sequence)",
                params![repo_root, next_sequence],
            )
            .context("failed to seed task sequence counter")?;
    }
    Ok(())
}

fn max_task_sequence(connection: &Connection, repo_root: &str) -> Result<u64> {
    let value: i64 = connection
        .query_row(
            "select coalesce(max(sequence), 0)
             from tasks
             where repo_root = ?1 and source not in ('agent', 'codex-session')",
            [repo_root],
            |row| row.get(0),
        )
        .context("failed to inspect task sequence max")?;
    u64::try_from(value).context("task sequence overflow")
}

fn repair_legacy_task_sequence_pollution_once(connection: &Connection) -> Result<()> {
    const MIGRATION: &str = "task_sequence_task_sources_only_v1";
    if schema_migration_applied(connection, MIGRATION)? {
        return Ok(());
    }
    scrub_non_task_sequences(connection)?;

    let mut statement = connection
        .prepare(
            "select repo_root
             from task_sequence_counters
             where trim(repo_root) != ''",
        )
        .context("failed to prepare task sequence counter repair")?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .context("failed to query task sequence counter repair")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode task sequence counter repair rows")?;
    drop(statement);

    for repo_root in rows {
        let repaired_next = max_task_sequence(connection, &repo_root)?.saturating_add(1);
        connection
            .execute(
                "update task_sequence_counters
                 set next_sequence = ?2
                 where repo_root = ?1",
                params![repo_root, i64::try_from(repaired_next).unwrap_or(i64::MAX)],
            )
            .context("failed to repair task sequence counter")?;
    }

    connection
        .execute(
            "insert into schema_migrations (name, applied_at_unix)
             values (?1, ?2)
             on conflict(name) do nothing",
            params![MIGRATION, i64::try_from(unix_now()).unwrap_or(i64::MAX)],
        )
        .context("failed to record task sequence repair migration")?;
    Ok(())
}

fn schema_migration_applied(connection: &Connection, name: &str) -> Result<bool> {
    let exists: i64 = connection
        .query_row(
            "select exists(select 1 from schema_migrations where name = ?1)",
            [name],
            |row| row.get(0),
        )
        .context("failed to inspect schema migration state")?;
    Ok(exists != 0)
}

fn scrub_non_task_sequences(connection: &Connection) -> Result<()> {
    connection
        .execute(
            "update tasks
             set sequence = null
             where source in ('agent', 'codex-session') and sequence is not null",
            [],
        )
        .context("failed to clear non-task sequence values")?;
    Ok(())
}

fn backfill_task_sequences(connection: &Connection) -> Result<()> {
    let mut statement = connection
        .prepare(
            "select id, source, repo_root
             from tasks
             where repo_root is not null and trim(repo_root) != '' and sequence is null
             order by repo_root, created_at_unix, id",
        )
        .context("failed to prepare task sequence backfill")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .context("failed to query task sequence backfill")?
        .collect::<Result<Vec<_>, _>>()
        .context("failed to decode task sequence backfill rows")?;
    drop(statement);

    for (id, source, repo_root) in rows {
        if !source_uses_task_sequence(&source) {
            continue;
        }
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
        execution_mode: row.get(2)?,
        selected_agent_command: row.get(3)?,
        selected_repo_root: row.get(4)?,
        selected_repo_name: row.get(5)?,
        track: row.get(6)?,
        current_index: row.get(7)?,
        stop_requested: row.get::<_, i64>(8)? != 0,
        message: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn queue_item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueItemRow> {
    let depends_on_json = row.get::<_, Option<String>>(15)?.unwrap_or_default();
    let depends_on = serde_json::from_str(&depends_on_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            15,
            rusqlite::types::Type::Text,
            Box::new(err),
        )
    })?;
    Ok(QueueItemRow {
        id: row.get(0)?,
        run_id: row.get(1)?,
        position: row.get(2)?,
        depends_on,
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

fn queue_depends_on_json(depends_on: &[String]) -> Result<String> {
    serde_json::to_string(depends_on).context("failed to encode queue dependencies")
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
