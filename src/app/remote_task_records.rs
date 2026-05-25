struct RemoteTaskRecordSyncSummary {
    via: String,
    local_repo_root: String,
    remote_repo_root: Option<String>,
    remote_records: usize,
    imported: usize,
    skipped: usize,
}

impl RemoteTaskRecordSyncSummary {
    fn render(&self) -> String {
        format!(
            "task-record-remote-sync\tvia={}\tlocal_repo={}\tremote_repo={}\tremote_records={}\
             \timported={}\tskipped={}",
            self.via,
            self.local_repo_root,
            self.remote_repo_root.as_deref().unwrap_or(""),
            self.remote_records,
            self.imported,
            self.skipped,
        )
    }
}

fn open_remote_task(args: &RemoteOpenArgs) -> Result<u8> {
    let record = record_task_open(&args.task_slug, args.profile.as_deref())?;
    let sequence = record
        .sequence
        .with_context(|| format!("task record {} did not receive a local sequence", record.id))?;
    let mut command = Command::new(&args.via);
    command
        .arg("env")
        .arg(format!("QCOLD_TASK_SEQUENCE={sequence}"));
    if let Some(prompt) = task_prompt_from_record(&record) {
        command.arg(format!("QCOLD_TASKFLOW_PROMPT={prompt}"));
    }
    if let Some(thread_id) = env_prompt("CODEX_THREAD_ID") {
        command.arg(format!("CODEX_THREAD_ID={thread_id}"));
        if let Some(path) = rollout::current_codex_rollout_path(Some(&thread_id)) {
            command.arg(format!("CODEX_ROLLOUT_PATH={}", path.display()));
        }
    }
    command
        .arg("qcold")
        .arg("task")
        .arg("open")
        .arg(&args.task_slug);
    if let Some(profile) = args.profile.as_deref() {
        command.arg(profile);
    }
    output_guard::scrub_inherited_output_guard(&mut command);
    let status = command
        .status()
        .with_context(|| format!("failed to run remote task launcher {}", args.via))?;
    Ok(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1))
}

fn sync_remote_task_records(args: &TaskRecordRemoteSyncArgs) -> Result<RemoteTaskRecordSyncSummary> {
    let local_repo_root = args
        .local_repo_root
        .clone()
        .map(|path| {
            path.canonicalize()
                .with_context(|| format!("failed to resolve local repo root {}", path.display()))
        })
        .transpose()?
        .map_or_else(repository::current_or_active_root, Ok)?;
    let local_repo_root = local_repo_root.display().to_string();
    let remote_records = remote_task_record_export(&args.via, args.limit)
        .or_else(|_| remote_task_record_list(&args.via, args.limit))?;
    let mut summary = RemoteTaskRecordSyncSummary {
        via: args.via.clone(),
        local_repo_root: local_repo_root.clone(),
        remote_repo_root: args.remote_repo_root.clone(),
        remote_records: remote_records.len(),
        imported: 0,
        skipped: 0,
    };
    for remote in remote_records {
        if args
            .remote_repo_root
            .as_deref()
            .is_some_and(|root| remote.repo_root.as_deref() != Some(root))
        {
            summary.skipped += 1;
            continue;
        }
        if remote.source != "task-flow" {
            summary.skipped += 1;
            continue;
        }
        let existing = state::get_task_record(&remote.id)?;
        let record = canonical_remote_task_record(&remote, existing.as_ref(), &local_repo_root);
        state::upsert_task_record(&record)?;
        summary.imported += 1;
    }
    Ok(summary)
}

fn remote_task_record_export(via: &str, limit: usize) -> Result<Vec<state::TaskRecordRow>> {
    let output = remote_qcold_output(via, &["task-record", "export", "--limit", &limit.to_string()])?;
    let mut records = Vec::new();
    for line in output.lines() {
        let Some(raw) = line.strip_prefix("task-record-json\t") else {
            continue;
        };
        records.push(serde_json::from_str::<state::TaskRecordRow>(raw)?);
    }
    Ok(records)
}

fn remote_task_record_list(via: &str, limit: usize) -> Result<Vec<state::TaskRecordRow>> {
    let output = remote_qcold_output(via, &["task-record", "list", "--limit", &limit.to_string()])?;
    Ok(output
        .lines()
        .filter_map(parse_rendered_task_record_line)
        .collect())
}

fn remote_qcold_output(via: &str, qcold_args: &[&str]) -> Result<String> {
    let mut command = Command::new(via);
    command.arg("qcold").args(qcold_args);
    output_guard::scrub_inherited_output_guard(&mut command);
    let output = command
        .output()
        .with_context(|| format!("failed to run remote Q-COLD through {via}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("remote Q-COLD exited with {}: {}", output.status, stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_rendered_task_record_line(line: &str) -> Option<state::TaskRecordRow> {
    let mut fields = line.split('\t');
    if fields.next()? != "task-record" {
        return None;
    }
    let id = fields.next()?.to_string();
    let mut values = BTreeMap::new();
    for field in fields {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        values.insert(key.to_string(), value.to_string());
    }
    Some(state::TaskRecordRow {
        id,
        sequence: values.get("sequence").and_then(|value| value.parse().ok()),
        status: values.get("status")?.clone(),
        source: values.get("source")?.clone(),
        title: values.get("title").cloned().unwrap_or_default(),
        description: String::new(),
        created_at: values
            .get("updated_at")
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(unix_now),
        updated_at: values
            .get("updated_at")
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(unix_now),
        repo_root: values.get("repo").cloned().filter(|value| !value.is_empty()),
        cwd: values.get("cwd").cloned().filter(|value| !value.is_empty()),
        agent_id: values.get("agent").cloned().filter(|value| !value.is_empty()),
        metadata_json: None,
    })
}

fn canonical_remote_task_record(
    remote: &state::TaskRecordRow,
    existing: Option<&state::TaskRecordRow>,
    local_repo_root: &str,
) -> state::TaskRecordRow {
    let mut record = existing.cloned().unwrap_or_else(|| {
        state::new_task_record(
            remote.id.clone(),
            remote.source.clone(),
            remote.title.clone(),
            if remote.description.trim().is_empty() {
                format!("Remote task-flow work for {}.", remote.title)
            } else {
                remote.description.clone()
            },
            remote.status.clone(),
            Some(local_repo_root.to_string()),
            remote.cwd.clone(),
            remote.agent_id.clone(),
            None,
        )
    });
    record.source.clone_from(&remote.source);
    record.title.clone_from(&remote.title);
    if !remote.description.trim().is_empty() {
        record.description.clone_from(&remote.description);
    }
    record.status = merged_remote_status(existing, &remote.status);
    record.updated_at = remote.updated_at;
    record.repo_root = Some(local_repo_root.to_string());
    record.cwd = remote.cwd.clone().or(record.cwd);
    record.agent_id = remote.agent_id.clone().or(record.agent_id);
    let mut metadata = remote
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if let Some(existing_metadata) = existing
        .and_then(|record| record.metadata_json.as_deref())
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
    {
        for (key, value) in existing_metadata {
            metadata.entry(key).or_insert(value);
        }
    }
    if let Some(remote_repo_root) = remote.repo_root.as_deref() {
        metadata.insert(
            "remote_repo_root".to_string(),
            Value::String(remote_repo_root.to_string()),
        );
    }
    if let Some(remote_cwd) = remote.cwd.as_deref() {
        metadata.insert(
            "remote_cwd".to_string(),
            Value::String(remote_cwd.to_string()),
        );
    }
    if let Some(sequence) = remote.sequence {
        metadata.insert("remote_sequence".to_string(), Value::from(sequence));
    }
    metadata.insert(
        "canonical_repo_root".to_string(),
        Value::String(local_repo_root.to_string()),
    );
    metadata.insert(
        "remote_status".to_string(),
        Value::String(remote.status.clone()),
    );
    metadata.insert("remote_synced_at".to_string(), Value::from(unix_now()));
    record.metadata_json = Some(Value::Object(metadata).to_string());
    record.sequence = existing.and_then(|record| record.sequence);
    record
}

fn merged_remote_status(existing: Option<&state::TaskRecordRow>, remote_status: &str) -> String {
    if let Some(existing) = existing {
        if task_record_terminal_status(&existing.status).is_some()
            && task_record_terminal_status(remote_status).is_none()
        {
            return existing.status.clone();
        }
    }
    remote_status.to_string()
}
