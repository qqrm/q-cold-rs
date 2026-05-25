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

pub fn delete_agent_record(id: &str) -> Result<bool> {
    let connection = open_db()?;
    let deleted = connection
        .execute("delete from agents where id = ?1", [id])
        .context("failed to delete agent record")?;
    Ok(deleted > 0)
}
