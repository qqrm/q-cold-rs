struct RemoteTaskRecordSyncSummary {
    via: String,
    remote_adapter: String,
    local_repo_root: String,
    remote_repo_root: Option<String>,
    remote_records: usize,
    imported: usize,
    skipped: usize,
}

impl RemoteTaskRecordSyncSummary {
    fn render(&self) -> String {
        format!(
            "task-record-remote-sync\tvia={}\tadapter={}\tlocal_repo={}\tremote_repo={}\
             \tremote_records={}\timported={}\tskipped={}",
            self.via,
            self.remote_adapter,
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
    let record = mark_remote_task_record(record, &args.remote, false)?;
    let sequence = record
        .sequence
        .with_context(|| format!("task record {} did not receive a local sequence", record.id))?;
    let mut command = Command::new(&args.remote.via);
    command.arg("env");
    command.args(remote_task_open_env_words(
        &record,
        &args.remote_env,
        sequence,
    ));
    append_remote_adapter_words(
        &mut command,
        &args.remote,
        remote_task_open_words(&args.task_slug),
    );
    if let Some(profile) = args.profile.as_deref() {
        command.arg(profile);
    }
    output_guard::scrub_inherited_output_guard(&mut command);
    let status = command
        .status()
        .with_context(|| format!("failed to run remote task launcher {}", args.remote.via))?;
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
    let remote_records = if args.legacy_remote_qcold {
        remote_qcold_task_record_export(&args.remote.via, args.limit)
            .or_else(|_| remote_qcold_task_record_list(&args.remote.via, args.limit))?
    } else {
        remote_adapter_task_record_export(&args.remote, args.limit)?
    };
    let mut summary = RemoteTaskRecordSyncSummary {
        via: args.remote.via.clone(),
        remote_adapter: if args.legacy_remote_qcold {
            "qcold".to_string()
        } else {
            remote_adapter_label(&args.remote)
        },
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
        let mut record = canonical_remote_task_record(&remote, existing.as_ref(), &local_repo_root);
        add_remote_adapter_metadata(&mut record, &args.remote, args.legacy_remote_qcold);
        state::upsert_task_record(&record)?;
        summary.imported += 1;
    }
    Ok(summary)
}

fn remote_adapter_task_record_export(
    remote: &RemoteAdapterArgs,
    limit: usize,
) -> Result<Vec<state::TaskRecordRow>> {
    let output = remote_adapter_output(
        remote,
        remote_task_record_export_words(limit),
        "remote repository task-record export",
    )?;
    parse_task_record_json_lines(&output)
}

fn remote_qcold_task_record_export(via: &str, limit: usize) -> Result<Vec<state::TaskRecordRow>> {
    let output = remote_qcold_output(via, &["task-record", "export", "--limit", &limit.to_string()])?;
    parse_task_record_json_lines(&output)
}

fn parse_task_record_json_lines(output: &str) -> Result<Vec<state::TaskRecordRow>> {
    let mut records = Vec::new();
    for line in output.lines() {
        let Some(raw) = line.strip_prefix("task-record-json\t") else {
            continue;
        };
        records.push(serde_json::from_str::<state::TaskRecordRow>(raw)?);
    }
    Ok(records)
}

fn remote_qcold_task_record_list(via: &str, limit: usize) -> Result<Vec<state::TaskRecordRow>> {
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

fn remote_adapter_output(
    remote: &RemoteAdapterArgs,
    adapter_args: Vec<OsString>,
    context: &str,
) -> Result<String> {
    let mut command = Command::new(&remote.via);
    append_remote_adapter_words(&mut command, remote, adapter_args);
    output_guard::scrub_inherited_output_guard(&mut command);
    let output = command
        .output()
        .with_context(|| format!("failed to run {context} through {}", remote.via))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{context} exited with {}: {}", output.status, stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn append_remote_adapter_words(
    command: &mut Command,
    remote: &RemoteAdapterArgs,
    adapter_args: Vec<OsString>,
) {
    command.arg(&remote.remote_adapter);
    for arg in remote_adapter_prefix_args(remote) {
        command.arg(arg);
    }
    command.args(adapter_args);
}

fn remote_adapter_prefix_args(remote: &RemoteAdapterArgs) -> Vec<OsString> {
    if remote.adapter_args.is_empty() && !remote.no_default_remote_adapter_arg {
        vec![OsString::from("xtask")]
    } else {
        remote.adapter_args.iter().map(OsString::from).collect()
    }
}

fn remote_task_open_env_words(
    record: &state::TaskRecordRow,
    env_args: &RemoteTaskOpenEnvArgs,
    sequence: u64,
) -> Vec<OsString> {
    let mut words = Vec::new();
    append_env_words(
        &mut words,
        "QCOLD_TASK_SEQUENCE",
        &env_args.sequence_vars,
        &sequence.to_string(),
    );
    if let Some(prompt) = task_prompt_from_record(record) {
        append_env_words(
            &mut words,
            "QCOLD_TASKFLOW_PROMPT",
            &env_args.prompt_names,
            &prompt,
        );
    }
    if !record.description.trim().is_empty() {
        append_env_words(
            &mut words,
            "QCOLD_TASKFLOW_DESCRIPTION",
            &env_args.description_keys,
            &record.description,
        );
    }
    if let Some(thread_id) = env_prompt("CODEX_THREAD_ID") {
        append_env_words(
            &mut words,
            "CODEX_THREAD_ID",
            &env_args.thread_targets,
            &thread_id,
        );
        if let Some(path) = rollout::current_codex_rollout_path(Some(&thread_id)) {
            append_env_words(
                &mut words,
                "CODEX_ROLLOUT_PATH",
                &env_args.rollout_targets,
                &path.display().to_string(),
            );
        }
    }
    words
}

fn append_env_words(words: &mut Vec<OsString>, primary: &str, aliases: &[String], value: &str) {
    words.push(env_word(primary, value));
    for alias in aliases {
        let alias = alias.trim();
        if !alias.is_empty() && alias != primary {
            words.push(env_word(alias, value));
        }
    }
}

fn env_word(name: &str, value: &str) -> OsString {
    OsString::from(format!("{name}={value}"))
}

fn remote_task_open_words(task_slug: &str) -> Vec<OsString> {
    vec![
        OsString::from("task"),
        OsString::from("open"),
        OsString::from(task_slug),
    ]
}

fn remote_task_record_export_words(limit: usize) -> Vec<OsString> {
    vec![
        OsString::from("task"),
        OsString::from("export-records"),
        OsString::from("--limit"),
        OsString::from(limit.to_string()),
    ]
}

fn remote_adapter_label(remote: &RemoteAdapterArgs) -> String {
    let mut words = vec![remote.remote_adapter.clone()];
    words.extend(
        remote_adapter_prefix_args(remote)
            .into_iter()
            .map(|arg| arg.to_string_lossy().to_string()),
    );
    words.join(" ")
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

fn mark_remote_task_record(
    mut record: state::TaskRecordRow,
    remote: &RemoteAdapterArgs,
    legacy_remote_qcold: bool,
) -> Result<state::TaskRecordRow> {
    add_remote_adapter_metadata(&mut record, remote, legacy_remote_qcold);
    state::upsert_task_record(&record)
}

fn add_remote_adapter_metadata(
    record: &mut state::TaskRecordRow,
    remote: &RemoteAdapterArgs,
    legacy_remote_qcold: bool,
) {
    let mut metadata = record
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    metadata.insert(
        "remote_launcher".to_string(),
        Value::String(remote.via.clone()),
    );
    metadata.insert(
        "remote_adapter".to_string(),
        Value::String(if legacy_remote_qcold {
            "qcold".to_string()
        } else {
            remote_adapter_label(remote)
        }),
    );
    metadata.insert(
        "remote_adapter_legacy_qcold".to_string(),
        Value::Bool(legacy_remote_qcold),
    );
    record.metadata_json = Some(Value::Object(metadata).to_string());
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
