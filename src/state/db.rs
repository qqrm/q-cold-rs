use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::queue_tabs::deduplicate_web_queue_tab_runs;
use super::{
    source_uses_task_sequence, AgentRow, QueueExecutionHost, QueueExecutionMode, QueueItemRow,
    QueueItemStatus, QueueRunRow, QueueRunStatus, QueueTabRow, QueueTaskClass, TaskRecordRow,
    TaskTopicRow, DEFAULT_SQLITE_BUSY_TIMEOUT_MS,
};

struct SchemaMigration {
    id: &'static str,
    apply: fn(&Connection) -> Result<()>,
}

static INITIALIZED_STATE_DB_PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

const SCHEMA_MIGRATIONS: &[SchemaMigration] = &[
    SchemaMigration {
        id: "001_initial_state_schema",
        apply: apply_initial_state_schema,
    },
    SchemaMigration {
        id: "002_task_record_repo_context",
        apply: apply_task_record_repo_context_schema,
    },
    SchemaMigration {
        id: "003_web_queue_tables",
        apply: apply_web_queue_schema,
    },
    SchemaMigration {
        id: "004_web_queue_execution_metadata",
        apply: apply_web_queue_execution_metadata_schema,
    },
    SchemaMigration {
        id: "005_task_sequence_counters",
        apply: apply_task_sequence_counter_schema,
    },
    SchemaMigration {
        id: "006_web_queue_worker_leases",
        apply: apply_web_queue_worker_lease_schema,
    },
    SchemaMigration {
        id: "007_web_queue_resource_admission",
        apply: apply_web_queue_resource_admission_schema,
    },
    SchemaMigration {
        id: "task_sequence_task_sources_only_v1",
        apply: repair_legacy_task_sequence_pollution,
    },
];

const INITIAL_STATE_SCHEMA_SQL: &str = "
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
        execution_host text not null default 'local',
        selected_agent_command text not null,
        remote_launcher text,
        remote_agent_local_proxy text,
        remote_agent_remote_proxy text,
        selected_repo_root text,
        selected_repo_name text,
        track text not null,
        current_index integer not null,
        stop_requested integer not null default 0,
        message text not null,
        created_at_unix integer not null,
        updated_at_unix integer not null
    );
    create table if not exists web_queue_tabs (
        id text primary key,
        label text not null,
        run_id text references web_queue_runs(id) on delete set null,
        is_default integer not null default 0,
        active integer not null default 0,
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
        execution_host text not null default 'local',
        agent_command text not null,
        task_class text not null default 'mid',
        remote_launcher text,
        remote_agent_local_proxy text,
        remote_agent_remote_proxy text,
        agent_id text,
        status text not null,
        message text not null,
        attempts integer not null default 0,
        recovery_attempts integer not null default 0,
        next_attempt_at_unix integer,
        started_at_unix integer not null,
        updated_at_unix integer not null,
        unique(run_id, position),
        unique(run_id, slug)
    );
    create table if not exists web_queue_item_attempts (
        run_id text not null,
        item_id text not null references web_queue_items(id) on delete cascade,
        semantic_iteration integer not null,
        agent_command text not null,
        agent_id text,
        task_record_id text,
        terminal_target text,
        stdout_log_path text,
        stderr_log_path text,
        bundle_path text,
        status text not null,
        failure_message text,
        started_at_unix integer not null,
        finished_at_unix integer,
        updated_at_unix integer not null,
        primary key(run_id, item_id, semantic_iteration)
    );
    create table if not exists web_queue_resource_samples (
        sampled_at_unix integer primary key,
        logical_cpus integer,
        load_one_milli integer,
        memory_total_bytes integer,
        memory_available_bytes integer,
        reserved_tasks integer not null,
        reserved_heavy_tasks integer not null
    );
    create unique index if not exists web_queue_tabs_default
    on web_queue_tabs(is_default)
    where is_default = 1;
    create unique index if not exists web_queue_tabs_active
    on web_queue_tabs(active)
    where active = 1;
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
    pragma user_version = 1;
";

pub(super) fn open_db() -> Result<Connection> {
    let path = db_path()?;
    let existed_before_open = path.is_file();
    fs::create_dir_all(
        path.parent()
            .context("state db path has no parent directory")?,
    )?;
    let mut connection =
        Connection::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
    connection
        .busy_timeout(sqlite_busy_timeout())
        .context("failed to set state database busy timeout")?;
    connection
        .execute_batch("pragma foreign_keys = on;")
        .context("failed to initialize state database connection pragmas")?;
    initialize_state_database_once(&mut connection, &path, existed_before_open)?;
    Ok(connection)
}

fn initialize_state_database_once(
    connection: &mut Connection,
    path: &Path,
    existed_before_open: bool,
) -> Result<()> {
    let initialized_paths = INITIALIZED_STATE_DB_PATHS.get_or_init(|| Mutex::new(HashSet::new()));
    let mut initialized_paths = initialized_paths
        .lock()
        .map_err(|_| anyhow::anyhow!("state database initialization lock is poisoned"))?;
    if !existed_before_open {
        initialized_paths.remove(path);
    }
    if initialized_paths.contains(path) {
        return Ok(());
    }
    initialize_state_database(connection)?;
    initialized_paths.insert(path.to_path_buf());
    Ok(())
}

fn initialize_state_database(connection: &mut Connection) -> Result<()> {
    connection
        .execute_batch(
            "pragma journal_mode = wal;
             pragma foreign_keys = on;",
        )
        .context("failed to initialize state database pragmas")?;
    apply_ordered_schema_migrations(connection)?;
    ensure_default_web_queue_tab(connection)?;
    scrub_non_task_sequences(connection)?;
    seed_task_sequence_counters(connection)?;
    backfill_task_sequences(connection)?;
    Ok(())
}

fn apply_ordered_schema_migrations(connection: &mut Connection) -> Result<()> {
    ensure_schema_migrations(connection)?;
    for migration in SCHEMA_MIGRATIONS {
        if schema_migration_applied(connection, migration.id)? {
            continue;
        }
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .with_context(|| format!("failed to begin schema migration {}", migration.id))?;
        (migration.apply)(&tx)
            .with_context(|| format!("failed to apply schema migration {}", migration.id))?;
        tx.execute(
            "insert into schema_migrations (name, applied_at_unix)
             values (?1, ?2)
             on conflict(name) do nothing",
            params![migration.id, i64::try_from(unix_now()).unwrap_or(i64::MAX)],
        )
        .with_context(|| format!("failed to record schema migration {}", migration.id))?;
        tx.commit()
            .with_context(|| format!("failed to commit schema migration {}", migration.id))?;
    }
    Ok(())
}

fn apply_initial_state_schema(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(INITIAL_STATE_SCHEMA_SQL)
        .context("failed to initialize state schema")?;
    Ok(())
}

fn apply_task_record_repo_context_schema(connection: &Connection) -> Result<()> {
    ensure_column(connection, "tasks", "repo_root", "text")?;
    ensure_column(connection, "tasks", "cwd", "text")?;
    ensure_column(connection, "tasks", "agent_id", "text")?;
    ensure_column(connection, "tasks", "metadata_json", "text")?;
    ensure_column(connection, "tasks", "sequence", "integer")?;
    ensure_column(connection, "agents", "cwd", "text")?;
    Ok(())
}

fn apply_web_queue_schema(connection: &Connection) -> Result<()> {
    ensure_web_queue_schema(connection)
}

fn apply_web_queue_execution_metadata_schema(connection: &Connection) -> Result<()> {
    ensure_column(
        connection,
        "web_queue_runs",
        "execution_mode",
        "text not null default 'sequence'",
    )?;
    ensure_column(
        connection,
        "web_queue_runs",
        "execution_host",
        "text not null default 'local'",
    )?;
    ensure_column(connection, "web_queue_runs", "remote_launcher", "text")?;
    ensure_column(
        connection,
        "web_queue_runs",
        "remote_agent_local_proxy",
        "text",
    )?;
    ensure_column(
        connection,
        "web_queue_runs",
        "remote_agent_remote_proxy",
        "text",
    )?;
    ensure_column(
        connection,
        "web_queue_items",
        "depends_on_json",
        "text not null default '[]'",
    )?;
    ensure_column(
        connection,
        "web_queue_items",
        "execution_host",
        "text not null default 'local'",
    )?;
    ensure_column(
        connection,
        "web_queue_items",
        "task_class",
        "text not null default 'mid'",
    )?;
    ensure_column(connection, "web_queue_items", "remote_launcher", "text")?;
    ensure_column(
        connection,
        "web_queue_items",
        "remote_agent_local_proxy",
        "text",
    )?;
    ensure_column(
        connection,
        "web_queue_items",
        "remote_agent_remote_proxy",
        "text",
    )?;
    ensure_column(
        connection,
        "web_queue_items",
        "recovery_attempts",
        "integer not null default 0",
    )?;
    ensure_web_queue_item_attempt_schema(connection)?;
    backfill_web_queue_item_attempts(connection)?;
    Ok(())
}

fn apply_task_sequence_counter_schema(connection: &Connection) -> Result<()> {
    connection
        .execute(
            "create table if not exists task_sequence_counters (
                 repo_root text primary key,
                 next_sequence integer not null
             )",
            [],
        )
        .context("failed to create task sequence counters table")?;
    connection
        .execute(
            "create unique index if not exists tasks_repo_sequence
             on tasks(repo_root, sequence)
             where repo_root is not null and sequence is not null",
            [],
        )
        .context("failed to create task sequence index")?;
    Ok(())
}

fn apply_web_queue_worker_lease_schema(connection: &Connection) -> Result<()> {
    ensure_web_queue_worker_lease_schema(connection)
}

fn apply_web_queue_resource_admission_schema(connection: &Connection) -> Result<()> {
    ensure_web_queue_resource_admission_schema(connection)
}

fn ensure_web_queue_schema(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            "create table if not exists web_queue_runs (
                 id text primary key,
                 status text not null,
                 execution_mode text not null default 'sequence',
                 execution_host text not null default 'local',
                 selected_agent_command text not null,
                 remote_launcher text,
                 remote_agent_local_proxy text,
                 remote_agent_remote_proxy text,
                 selected_repo_root text,
                 selected_repo_name text,
                 track text not null,
                 current_index integer not null,
                 stop_requested integer not null default 0,
                 message text not null,
                 created_at_unix integer not null,
                 updated_at_unix integer not null
             );
             create table if not exists web_queue_tabs (
                 id text primary key,
                 label text not null,
                 run_id text references web_queue_runs(id) on delete set null,
                 is_default integer not null default 0,
                 active integer not null default 0,
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
                 execution_host text not null default 'local',
                 agent_command text not null,
                 task_class text not null default 'mid',
                 remote_launcher text,
                 remote_agent_local_proxy text,
                 remote_agent_remote_proxy text,
                 agent_id text,
                 status text not null,
                 message text not null,
                 attempts integer not null default 0,
                 recovery_attempts integer not null default 0,
                 next_attempt_at_unix integer,
                 started_at_unix integer not null,
                 updated_at_unix integer not null,
                 unique(run_id, position),
                 unique(run_id, slug)
             );
             create table if not exists web_queue_item_attempts (
                 run_id text not null,
                 item_id text not null references web_queue_items(id) on delete cascade,
                 semantic_iteration integer not null,
                 agent_command text not null,
                 agent_id text,
                 task_record_id text,
                 terminal_target text,
                 stdout_log_path text,
                 stderr_log_path text,
                 bundle_path text,
                 status text not null,
                 failure_message text,
                 started_at_unix integer not null,
                 finished_at_unix integer,
                 updated_at_unix integer not null,
                 primary key(run_id, item_id, semantic_iteration)
             );
             create table if not exists web_queue_resource_samples (
                 sampled_at_unix integer primary key,
                 logical_cpus integer,
                 load_one_milli integer,
                 memory_total_bytes integer,
                 memory_available_bytes integer,
                 reserved_tasks integer not null,
                 reserved_heavy_tasks integer not null
             );
             create unique index if not exists web_queue_tabs_default
             on web_queue_tabs(is_default)
             where is_default = 1;
             create unique index if not exists web_queue_tabs_active
             on web_queue_tabs(active)
             where active = 1;",
        )
        .context("failed to initialize web queue tables")?;
    Ok(())
}

fn ensure_web_queue_resource_admission_schema(connection: &Connection) -> Result<()> {
    ensure_column(
        connection,
        "web_queue_items",
        "task_class",
        "text not null default 'mid'",
    )?;
    connection
        .execute(
            "create table if not exists web_queue_resource_samples (
                 sampled_at_unix integer primary key,
                 logical_cpus integer,
                 load_one_milli integer,
                 memory_total_bytes integer,
                 memory_available_bytes integer,
                 reserved_tasks integer not null,
                 reserved_heavy_tasks integer not null
             )",
            [],
        )
        .context("failed to initialize web queue resource sample table")?;
    Ok(())
}

fn ensure_web_queue_worker_lease_schema(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            "create table if not exists web_queue_item_worker_leases (
                 item_id text primary key references web_queue_items(id) on delete cascade,
                 run_id text not null references web_queue_runs(id) on delete cascade,
                 owner_id text,
                 lease_epoch integer not null default 0,
                 acquired_at_unix integer,
                 heartbeat_at_unix integer,
                 expires_at_unix integer,
                 released_at_unix integer,
                 recovery_count integer not null default 0,
                 updated_at_unix integer not null
             );
             create index if not exists web_queue_item_worker_leases_run
             on web_queue_item_worker_leases(run_id);
             create index if not exists web_queue_item_worker_leases_active
             on web_queue_item_worker_leases(expires_at_unix)
             where owner_id is not null;",
        )
        .context("failed to initialize web queue worker lease table")?;
    Ok(())
}

fn ensure_web_queue_item_attempt_schema(connection: &Connection) -> Result<()> {
    connection
        .execute(
            "create table if not exists web_queue_item_attempts (
                 run_id text not null,
                 item_id text not null references web_queue_items(id) on delete cascade,
                 semantic_iteration integer not null,
                 agent_command text not null,
                 agent_id text,
                 task_record_id text,
                 terminal_target text,
                 stdout_log_path text,
                 stderr_log_path text,
                 bundle_path text,
                 status text not null,
                 failure_message text,
                 started_at_unix integer not null,
                 finished_at_unix integer,
                 updated_at_unix integer not null,
                 primary key(run_id, item_id, semantic_iteration)
             )",
            [],
        )
        .context("failed to create web queue item attempt ledger")?;
    Ok(())
}

fn backfill_web_queue_item_attempts(connection: &Connection) -> Result<()> {
    let now = unix_now();
    connection
        .execute(
            "insert into web_queue_item_attempts
                 (run_id, item_id, semantic_iteration, agent_command, agent_id, task_record_id,
                  status, failure_message, started_at_unix, finished_at_unix, updated_at_unix)
             select run_id, id, recovery_attempts + 1, agent_command, agent_id, 'task/' || slug,
                    status,
                    case when status in ('failed', 'blocked') then nullif(message, '') else null end,
                    started_at_unix,
                    case when status in ('success', 'failed', 'blocked') then updated_at_unix else null end,
                    ?1
             from web_queue_items
             where true
             on conflict(run_id, item_id, semantic_iteration) do nothing",
            [now],
        )
        .context("failed to backfill web queue item attempt ledger")?;
    Ok(())
}

pub(super) fn ensure_default_web_queue_tab(connection: &Connection) -> Result<()> {
    let default_exists = connection
        .query_row(
            "select 1 from web_queue_tabs where is_default = 1 limit 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to query default web queue tab")?
        .is_some();
    if !default_exists {
        let run_id = latest_web_queue_run_id(connection)?;
        let now = unix_now();
        connection
            .execute(
                "insert into web_queue_tabs
                     (id, label, run_id, is_default, active, created_at_unix, updated_at_unix)
                 values ('default', 'Task Queue', ?1, 1, 1, ?2, ?2)",
                params![run_id, now],
            )
            .context("failed to create default web queue tab")?;
    }
    let active_exists = connection
        .query_row(
            "select 1 from web_queue_tabs where active = 1 limit 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to query active web queue tab")?
        .is_some();
    if !active_exists {
        let fallback = active_fallback_queue_tab_id(connection)?;
        connection
            .execute(
                "update web_queue_tabs set active = 1, updated_at_unix = ?2 where id = ?1",
                params![fallback, unix_now()],
            )
            .context("failed to activate default web queue tab")?;
    }
    deduplicate_web_queue_tab_runs(connection)?;
    Ok(())
}

fn latest_web_queue_run_id(connection: &Connection) -> Result<Option<String>> {
    connection
        .query_row(
            "select id from web_queue_runs order by updated_at_unix desc limit 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to query latest web queue run")
}

pub(super) fn active_fallback_queue_tab_id(connection: &Connection) -> Result<String> {
    connection
        .query_row(
            "select id
             from web_queue_tabs
             order by case when run_id is not null then 0 else 1 end,
                      is_default desc,
                      created_at_unix,
                      id
             limit 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to query fallback queue tab")?
        .context("missing fallback queue tab")
}

pub(super) fn active_web_queue_run_id(connection: &Connection) -> Result<Option<String>> {
    ensure_default_web_queue_tab(connection)?;
    connection
        .query_row(
            "select run_id from web_queue_tabs where active = 1 limit 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to query active web queue run")
        .map(Option::flatten)
}

pub(super) fn assign_web_queue_run_to_active_tab(
    connection: &Connection,
    run_id: &str,
) -> Result<()> {
    ensure_default_web_queue_tab(connection)?;
    connection
        .execute(
            "update web_queue_tabs
             set run_id = null, updated_at_unix = ?2
             where active = 0 and run_id = ?1",
            params![run_id, unix_now()],
        )
        .context("failed to clear duplicate queue run from inactive tabs")?;
    let updated = connection
        .execute(
            "update web_queue_tabs
             set run_id = ?1, updated_at_unix = ?2
             where active = 1",
            params![run_id, unix_now()],
        )
        .context("failed to assign queue run to active tab")?;
    if updated == 0 {
        bail!("no active queue tab");
    }
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

pub(super) fn task_record_sequence_for_upsert(
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
        (_, Some(sequence), _) | (Some(sequence), _, _) => Ok(Some(sequence)),
        (None, None, Some(repo_root)) if !repo_root.trim().is_empty() => {
            allocate_task_sequence(connection, repo_root).map(Some)
        }
        _ => Ok(None),
    }
}

pub(super) fn advance_task_sequence_counter(
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
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
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

fn repair_legacy_task_sequence_pollution(connection: &Connection) -> Result<()> {
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

pub(super) fn backfill_agents(connection: &Connection, legacy_path: &Path) -> Result<()> {
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

pub(super) fn backfill_task_topics(connection: &Connection, legacy_path: &Path) -> Result<()> {
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

pub(super) fn backfill_task_events(
    connection: &Connection,
    legacy_events_dir: &Path,
) -> Result<()> {
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

pub(super) fn task_topic_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskTopicRow> {
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

pub(super) fn task_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecordRow> {
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

pub(super) fn queue_run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueRunRow> {
    Ok(QueueRunRow {
        id: row.get(0)?,
        status: QueueRunStatus::from_db_value(row.get::<_, String>(1)?),
        execution_mode: QueueExecutionMode::from_db_value(row.get::<_, String>(2)?),
        execution_host: QueueExecutionHost::from_db_value(row.get::<_, String>(3)?),
        selected_agent_command: row.get(4)?,
        remote_launcher: row.get(5)?,
        remote_agent_local_proxy: row.get(6)?,
        remote_agent_remote_proxy: row.get(7)?,
        selected_repo_root: row.get(8)?,
        selected_repo_name: row.get(9)?,
        track: row.get(10)?,
        current_index: row.get(11)?,
        stop_requested: row.get::<_, i64>(12)? != 0,
        message: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

pub(super) fn queue_tab_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueTabRow> {
    Ok(QueueTabRow {
        id: row.get(0)?,
        label: row.get(1)?,
        run_id: row.get(2)?,
        is_default: row.get::<_, i64>(3)? != 0,
        active: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

pub(super) fn queue_item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueItemRow> {
    let depends_on_json = row.get::<_, Option<String>>(21)?.unwrap_or_default();
    let depends_on = serde_json::from_str(&depends_on_json).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(21, rusqlite::types::Type::Text, Box::new(err))
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
        execution_host: QueueExecutionHost::from_db_value(row.get::<_, String>(7)?),
        agent_command: row.get(8)?,
        task_class: QueueTaskClass::from_db_value(row.get::<_, String>(9)?),
        remote_launcher: row.get(10)?,
        remote_agent_local_proxy: row.get(11)?,
        remote_agent_remote_proxy: row.get(12)?,
        agent_id: row.get(13)?,
        status: QueueItemStatus::from_db_value(row.get::<_, String>(14)?),
        message: row.get(15)?,
        attempts: row.get(16)?,
        recovery_attempts: row.get(17)?,
        next_attempt_at: row.get(18)?,
        started_at: row.get(19)?,
        updated_at: row.get(20)?,
    })
}

pub(super) fn remove_web_queue_dependency_references(
    connection: &Connection,
    run_id: &str,
    deleted_item_id: &str,
) -> Result<()> {
    let mut statement = connection
        .prepare("select id, depends_on_json from web_queue_items where run_id = ?1")
        .context("failed to prepare queue dependency cleanup query")?;
    let rows = statement
        .query_map([run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("failed to query queue dependencies for cleanup")?;
    let mut updates = Vec::new();
    for row in rows {
        let (id, depends_on_json) = row.context("failed to read queue dependency row")?;
        let mut depends_on = serde_json::from_str::<Vec<String>>(&depends_on_json)
            .context("failed to decode queue dependency row")?;
        let old_len = depends_on.len();
        depends_on.retain(|dependency| dependency != deleted_item_id);
        if depends_on.len() != old_len {
            updates.push((id, queue_depends_on_json(&depends_on)?));
        }
    }
    drop(statement);
    let now = unix_now();
    for (id, depends_on_json) in updates {
        connection.execute(
            "update web_queue_items set depends_on_json = ?3, updated_at_unix = ?4 where run_id = ?1 and id = ?2",
            params![run_id, id, depends_on_json, now],
        )?;
    }
    Ok(())
}

pub(super) fn queue_depends_on_json(depends_on: &[String]) -> Result<String> {
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

pub(crate) fn state_dir() -> Result<PathBuf> {
    if let Ok(path) = env::var("QCOLD_STATE_DIR") {
        if !path.trim().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    let home = env::var("HOME").context("HOME is required when QCOLD_STATE_DIR is unset")?;
    Ok(PathBuf::from(home).join(".local/state/qcold"))
}

fn sqlite_busy_timeout() -> Duration {
    Duration::from_millis(
        env::var("QCOLD_SQLITE_BUSY_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_SQLITE_BUSY_TIMEOUT_MS),
    )
}

pub(super) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn unescape_field(value: &str) -> String {
    value.replace("\\t", "\t").replace("\\\\", "\\")
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::{initialize_state_database, SCHEMA_MIGRATIONS};

    const REPRESENTATIVE_LEGACY_SCHEMA_SQL: &str = "
        create table agents (
            id text primary key,
            track text not null,
            pid integer not null,
            started_at_unix integer not null,
            command_json text not null,
            stdout_log_path text,
            stderr_log_path text,
            created_at_unix integer not null
        );
        create table tasks (
            id text primary key,
            source text not null,
            title text not null,
            description text not null,
            status text not null,
            created_at_unix integer not null,
            updated_at_unix integer not null
        );
        create table web_queue_runs (
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
        create table web_queue_items (
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
    ";

    #[test]
    fn representative_legacy_schema_migrates_queue_data_and_records_migrations() {
        let mut connection = Connection::open_in_memory().unwrap();
        create_representative_legacy_schema(&connection);
        insert_legacy_queue_data(&connection);

        initialize_state_database(&mut connection).unwrap();

        let task_count: i64 = connection
            .query_row(
                "select count(*) from tasks where id = 'task/legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(task_count, 1);

        let run = connection
            .query_row(
                "select execution_mode, execution_host, remote_launcher
                 from web_queue_runs
                 where id = 'run-legacy'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(run, ("sequence".to_string(), "local".to_string(), None));

        let item = connection
            .query_row(
                "select depends_on_json, execution_host, recovery_attempts
                 from web_queue_items
                 where run_id = 'run-legacy' and id = 'item-legacy'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(item, ("[]".to_string(), "local".to_string(), 0));

        let lease_rows: i64 = connection
            .query_row(
                "select count(*) from web_queue_item_worker_leases",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(lease_rows, 0);

        let default_tab = connection
            .query_row(
                "select run_id, active, is_default
                 from web_queue_tabs
                 where id = 'default'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(default_tab, (Some("run-legacy".to_string()), 1, 1));

        for migration in SCHEMA_MIGRATIONS {
            assert!(migration_recorded(&connection, migration.id));
        }
    }

    #[test]
    fn fresh_and_migrated_schema_have_equivalent_table_column_and_index_coverage() {
        let mut fresh = Connection::open_in_memory().unwrap();
        initialize_state_database(&mut fresh).unwrap();

        let mut migrated = Connection::open_in_memory().unwrap();
        create_representative_legacy_schema(&migrated);
        initialize_state_database(&mut migrated).unwrap();

        assert_eq!(schema_signature(&fresh), schema_signature(&migrated));
    }

    #[test]
    fn schema_migration_registry_is_idempotent() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_state_database(&mut connection).unwrap();
        let schema = schema_signature(&connection);
        let migration_count = recorded_migration_count(&connection);

        initialize_state_database(&mut connection).unwrap();

        assert_eq!(schema_signature(&connection), schema);
        assert_eq!(recorded_migration_count(&connection), migration_count);
        assert_eq!(migration_count, SCHEMA_MIGRATIONS.len());
    }

    fn create_representative_legacy_schema(connection: &Connection) {
        connection
            .execute_batch(REPRESENTATIVE_LEGACY_SCHEMA_SQL)
            .unwrap();
    }

    fn insert_legacy_queue_data(connection: &Connection) {
        connection
            .execute(
                "insert into tasks
                     (id, source, title, description, status, created_at_unix, updated_at_unix)
                 values ('task/legacy', 'manual', 'Legacy', 'Legacy task', 'open', 10, 10)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "insert into web_queue_runs
                     (id, status, selected_agent_command, selected_repo_root, selected_repo_name,
                      track, current_index, stop_requested, message, created_at_unix, updated_at_unix)
                 values ('run-legacy', 'running', 'c1', '/repo', 'repo', 'track', 0, 0,
                         'running', 20, 30)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "insert into web_queue_items
                     (id, run_id, position, prompt, slug, repo_root, repo_name, agent_command,
                      agent_id, status, message, attempts, next_attempt_at_unix, started_at_unix,
                      updated_at_unix)
                 values ('item-legacy', 'run-legacy', 0, 'prompt', 'legacy', '/repo', 'repo',
                         'c1', null, 'pending', 'pending', 0, null, 20, 30)",
                [],
            )
            .unwrap();
    }

    fn migration_recorded(connection: &Connection, id: &str) -> bool {
        connection
            .query_row(
                "select exists(select 1 from schema_migrations where name = ?1)",
                [id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            != 0
    }

    fn recorded_migration_count(connection: &Connection) -> usize {
        let count: i64 = connection
            .query_row("select count(*) from schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        usize::try_from(count).unwrap()
    }

    fn schema_signature(connection: &Connection) -> Vec<String> {
        let mut entries = Vec::new();
        for table in schema_tables(connection) {
            entries.push(format!("table:{table}"));
            entries.extend(table_columns(connection, &table));
        }
        entries.extend(schema_indexes(connection));
        entries.sort();
        entries
    }

    fn schema_tables(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare(
                "select name
                 from sqlite_schema
                 where type = 'table' and name not like 'sqlite_%'
                 order by name",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn table_columns(connection: &Connection, table: &str) -> Vec<String> {
        let mut statement = connection
            .prepare(&format!("pragma table_info({table})"))
            .unwrap();
        statement
            .query_map([], |row| {
                let name = row.get::<_, String>(1)?;
                let type_name = row.get::<_, String>(2)?;
                let not_null = row.get::<_, i64>(3)?;
                let default_value = row.get::<_, Option<String>>(4)?.unwrap_or_default();
                let primary_key = row.get::<_, i64>(5)?;
                Ok(format!(
                    "column:{table}:{name}:{type_name}:{not_null}:{default_value}:{primary_key}"
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn schema_indexes(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare(
                "select name, tbl_name, sql
                 from sqlite_schema
                 where type = 'index' and name not like 'sqlite_autoindex_%'
                 order by name",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                let name = row.get::<_, String>(0)?;
                let table = row.get::<_, String>(1)?;
                let sql = row.get::<_, Option<String>>(2)?.unwrap_or_default();
                Ok(format!("index:{name}:{table}:{}", normalize_sql(&sql)))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn normalize_sql(sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}
