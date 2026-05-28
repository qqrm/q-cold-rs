const QUEUE_PROCESS_OUTPUT_LIMIT: usize = 1200;

struct QueueLaunchWorkspace {
    worktree: PathBuf,
    remote_launcher: Option<String>,
    remote_worktree: Option<String>,
    existing_task: bool,
}

fn queue_item_remote_native(item: &state::QueueItemRow) -> bool {
    item.execution_host == "remote-native"
}

fn queue_launch_workspace(item: &state::QueueItemRow) -> Result<QueueLaunchWorkspace> {
    let repo_root = queue_item_repo_root(item)?;
    ensure_queue_item_slug_available(item)?;
    ensure_queue_task_record_scope_available(item)?;
    let remote_launcher = item.remote_launcher.clone();
    if let Some(launcher) = remote_launcher.as_deref() {
        if let Some(task) = existing_remote_queue_task(item, launcher, &repo_root)? {
            return Ok(task);
        }
    }
    if let Some(worktree) = existing_queue_task_worktree(item, &repo_root)? {
        return Ok(QueueLaunchWorkspace {
            worktree,
            remote_launcher,
            remote_worktree: None,
            existing_task: true,
        });
    }
    Ok(QueueLaunchWorkspace {
        worktree: repo_root,
        remote_launcher,
        remote_worktree: None,
        existing_task: false,
    })
}

fn start_remote_native_queue_item(
    run_id: &str,
    item: &state::QueueItemRow,
    attempts: i64,
) -> Result<QueueItemOutcome> {
    let repo_root = queue_item_repo_root(item)?;
    ensure_queue_item_slug_available(item)?;
    ensure_queue_task_record_scope_available(item)?;
    let launcher = item
        .remote_launcher
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("remote-native queue item requires remote_launcher or selected_remote_launcher")?;
    let agent_id = queue_agent_id(item);
    let session = remote_native_queue_session(&agent_id);
    state::update_web_queue_item(
        run_id,
        &item.id,
        "starting",
        "checking remote-agent prerequisites",
        Some(&agent_id),
        attempts,
        None,
    )?;
    if let Err(err) = run_remote_agent_contract(item, &repo_root, "doctor", &session, None) {
        return Ok(QueueItemOutcome::retryable_failure(format!("{err:#}")));
    }
    state::update_web_queue_item(
        run_id,
        &item.id,
        "starting",
        "opening remote-native agent",
        Some(&agent_id),
        attempts,
        None,
    )?;
    if let Err(err) = run_remote_agent_contract(item, &repo_root, "open", &session, Some(&item.slug)) {
        return Ok(QueueItemOutcome::retryable_failure(format!("{err:#}")));
    }
    thread::sleep(Duration::from_secs(4));
    let instruction = queue_remote_native_task_instruction(item);
    if let Err(err) = send_remote_native_instruction(launcher, &repo_root, &session, &instruction) {
        return Ok(QueueItemOutcome::retryable_failure(format!("{err:#}")));
    }
    state::update_web_queue_item(
        run_id,
        &item.id,
        "running",
        &format!("{} ({agent_id})", queue_display_label(item)),
        Some(&agent_id),
        attempts,
        None,
    )?;
    let wait_item = remote_native_running_wait_item(item);
    wait_for_queue_item_closeout(run_id, &wait_item, &agent_id, attempts)
}

fn remote_native_queue_session(agent_id: &str) -> String {
    format!("qcold-{agent_id}")
}

fn remote_native_running_wait_item(item: &state::QueueItemRow) -> state::QueueItemRow {
    let mut item = item.clone();
    item.status = "running".to_string();
    item
}

fn run_remote_agent_contract(
    item: &state::QueueItemRow,
    repo_root: &Path,
    subcommand: &str,
    session: &str,
    task_slug: Option<&str>,
) -> Result<()> {
    let mut command = Command::new("cargo");
    command
        .current_dir(repo_root)
        .arg("xtask")
        .arg("remote-agent")
        .arg(subcommand)
        .arg("--session")
        .arg(session);
    if let Some(launcher) = item.remote_launcher.as_deref() {
        set_remote_agent_launcher_env(&mut command, launcher);
    }
    if let Some(proxy) = item.remote_agent_local_proxy.as_deref() {
        command.arg("--local-proxy").arg(proxy);
    }
    if let Some(proxy) = item.remote_agent_remote_proxy.as_deref() {
        command.arg("--remote-proxy").arg(proxy);
    }
    if let Some(slug) = task_slug {
        command.arg(slug);
    }
    let output = command
        .output()
        .with_context(|| format!("failed to run repository remote-agent {subcommand} contract"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "repository remote-agent {subcommand} contract failed with {}: {}",
            output.status,
            compact_process_output(&stdout, &stderr)
        );
    }
    Ok(())
}

fn set_remote_agent_launcher_env(command: &mut Command, launcher: &str) {
    command.env("QCOLD_REMOTE_DEV_ENV_WRAPPER", launcher);
    if let Some(name) = remote_agent_launcher_env_alias() {
        command.env(name, launcher);
    }
}

fn remote_agent_launcher_env_alias() -> Option<String> {
    let name = env::var("QCOLD_QUEUE_REMOTE_AGENT_LAUNCHER_ENV").ok()?;
    let name = name.trim();
    if name.is_empty() || name.contains('=') || name.contains('\0') {
        return None;
    }
    Some(name.to_string())
}

fn send_remote_native_instruction(
    launcher: &str,
    repo_root: &Path,
    session: &str,
    instruction: &str,
) -> Result<()> {
    let target = format!("{session}:0.0");
    let buffer = format!("{session}-task-packet");
    let script = format!(
        "tmux load-buffer -b {} -w - && tmux paste-buffer -b {} -t {} && tmux send-keys -t {} C-m",
        queue_shell_quote(&buffer),
        queue_shell_quote(&buffer),
        queue_shell_quote(&target),
        queue_shell_quote(&target),
    );
    let mut child = Command::new(launcher)
        .current_dir(repo_root)
        .args(["bash", "-lc", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to send prompt through remote launcher {launcher}"))?;
    {
        let stdin = child.stdin.as_mut().context("remote launcher stdin was not piped")?;
        stdin
            .write_all(instruction.as_bytes())
            .context("failed to write remote-native task packet")?;
    }
    let output = child
        .wait_with_output()
        .context("failed to wait for remote-native task packet send")?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "remote-native task packet send through {launcher} failed with {}: {}",
            output.status,
            compact_process_output(&stdout, &stderr)
        );
    }
    Ok(())
}

fn queue_qcold_executable() -> Result<PathBuf> {
    let current = env::current_exe().context("failed to locate current Q-COLD executable")?;
    queue_qcold_executable_from(&current, env::var_os("PATH").as_deref())
}

fn queue_qcold_executable_from(current: &Path, path: Option<&std::ffi::OsStr>) -> Result<PathBuf> {
    if executable_file(current) {
        return Ok(current.to_path_buf());
    }
    if let Some(installed) = qcold_executable_from_path(path) {
        return Ok(installed);
    }
    bail!(
        "failed to locate runnable Q-COLD executable; current executable {} is unavailable and \
         qcold was not found on PATH",
        current.display()
    );
}

fn qcold_executable_from_path(path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let binary = format!("qcold{}", env::consts::EXE_SUFFIX);
    env::split_paths(path?).find_map(|directory| {
        let candidate = directory.join(&binary);
        executable_file(&candidate).then_some(candidate)
    })
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && has_execute_permission(&metadata)
}

#[cfg(unix)]
fn has_execute_permission(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn has_execute_permission(_metadata: &fs::Metadata) -> bool {
    true
}

fn existing_queue_task_worktree(
    item: &state::QueueItemRow,
    repo_root: &Path,
) -> Result<Option<PathBuf>> {
    let task_id = format!("task/{}", item.slug);
    if let Some(record) = state::get_task_record(&task_id)? {
        if queue_task_record_matches_item(item, &record) {
            if let Some(cwd) = record.cwd.as_deref().map(PathBuf::from) {
                if validate_queue_task_worktree(&cwd, &item.slug, repo_root).is_ok() {
                    return Ok(Some(cwd));
                }
            }
        }
    }
    if let Some(worktree) = discover_queue_task_worktree(repo_root, &item.slug) {
        return Ok(Some(worktree));
    }
    Ok(None)
}

fn existing_remote_queue_task(
    item: &state::QueueItemRow,
    launcher: &str,
    repo_root: &Path,
) -> Result<Option<QueueLaunchWorkspace>> {
    let task_id = format!("task/{}", item.slug);
    let Some(record) = state::get_task_record(&task_id)? else {
        return Ok(None);
    };
    if !queue_task_record_matches_item(item, &record) {
        return Ok(None);
    }
    let metadata = task_record_metadata_object(&record);
    let remote_launcher = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("remote_launcher"))
        .and_then(Value::as_str);
    if remote_launcher != Some(launcher) {
        return Ok(None);
    }
    let remote_worktree = metadata
        .as_ref()
        .and_then(|metadata| metadata.get("remote_cwd"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(record.cwd);
    let Some(remote_worktree) = remote_worktree.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    Ok(Some(QueueLaunchWorkspace {
        worktree: repo_root.to_path_buf(),
        remote_launcher: Some(launcher.to_string()),
        remote_worktree: Some(remote_worktree),
        existing_task: true,
    }))
}

fn discover_queue_task_worktree(repo_root: &Path, task_slug: &str) -> Option<PathBuf> {
    let repo_root = repo_root.canonicalize().ok()?;
    let inventory_root = repo_root.parent()?.join("WT").join(repo_root.file_name()?);
    for entry in fs::read_dir(inventory_root).ok()? {
        let path = entry.ok()?.path();
        if validate_queue_task_worktree(&path, task_slug, &repo_root).is_ok() {
            return Some(path);
        }
    }
    None
}

fn validate_queue_task_worktree(path: &Path, task_slug: &str, repo_root: &Path) -> Result<()> {
    let env_path = path.join(".task/task.env");
    let content = fs::read_to_string(&env_path)
        .with_context(|| format!("missing task metadata at {}", env_path.display()))?;
    if task_env_value(&content, "TASK_NAME").as_deref() != Some(task_slug) {
        bail!("{} does not describe task {task_slug}", env_path.display());
    }
    let primary_repo = task_env_value(&content, "PRIMARY_REPO_PATH")
        .with_context(|| format!("{} has no PRIMARY_REPO_PATH", env_path.display()))?;
    if !same_filesystem_path(&primary_repo, &repo_root.display().to_string()) {
        bail!(
            "{} belongs to repository {primary_repo}, not {}",
            env_path.display(),
            repo_root.display()
        );
    }
    Ok(())
}

fn task_env_value(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    content.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .map(shell_env_value)
            .filter(|value| !value.trim().is_empty())
    })
}

fn shell_env_value(raw: &str) -> String {
    if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
        return raw[1..raw.len() - 1].replace("'\\''", "'");
    }
    raw.to_string()
}

fn queue_shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':' | b'='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn queue_item_repo_root(item: &state::QueueItemRow) -> Result<PathBuf> {
    let repo_root = item
        .repo_root
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .context("queue item has no repository root")?;
    PathBuf::from(repo_root)
        .canonicalize()
        .with_context(|| format!("failed to resolve queue repository {repo_root}"))
}

fn task_record_metadata_object(
    record: &state::TaskRecordRow,
) -> Option<serde_json::Map<String, Value>> {
    record
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
}

fn remember_queue_task_agent(item: &state::QueueItemRow, agent_id: &str) -> Result<()> {
    let task_id = format!("task/{}", item.slug);
    let Some(mut record) = state::get_task_record(&task_id)? else {
        return Ok(());
    };
    if !queue_task_record_matches_item(item, &record) {
        return Ok(());
    }
    record.agent_id = Some(agent_id.to_string());
    state::upsert_task_record(&record)?;
    Ok(())
}

fn ensure_queue_item_slug_available(item: &state::QueueItemRow) -> Result<()> {
    let Some(conflict) = state::load_web_queue_items()?.into_iter().find(|other| {
        other.slug == item.slug
            && (other.run_id != item.run_id || other.id != item.id)
            && !queue_item_terminal(&other.status)
    }) else {
        return Ok(());
    };
    bail!(
        "queue task slug task/{} is already active in run {}; choose a different slug",
        item.slug,
        conflict.run_id
    )
}

fn ensure_queue_task_record_scope_available(item: &state::QueueItemRow) -> Result<()> {
    let task_id = format!("task/{}", item.slug);
    let Some(record) = state::get_task_record(&task_id)? else {
        return Ok(());
    };
    if queue_task_record_matches_item(item, &record) || queue_task_record_is_terminal(&record) {
        return Ok(());
    }
    bail!(
        "queue task slug {task_id} already belongs to {}; choose a different slug",
        task_record_scope_summary(&record)
    )
}

fn queue_task_record_matches_item(
    item: &state::QueueItemRow,
    record: &state::TaskRecordRow,
) -> bool {
    (queue_task_record_repo_matches_item(item, record)
        && queue_task_record_launcher_matches_item(item, record))
        || queue_task_record_agent_matches_item(item, record)
}

fn queue_task_record_agent_matches_item(
    item: &state::QueueItemRow,
    record: &state::TaskRecordRow,
) -> bool {
    item.agent_id
        .as_deref()
        .zip(record.agent_id.as_deref())
        .is_some_and(|(item_agent, record_agent)| item_agent == record_agent)
}

fn queue_task_record_repo_matches_item(
    item: &state::QueueItemRow,
    record: &state::TaskRecordRow,
) -> bool {
    let Some(item_repo) = item.repo_root.as_deref().filter(|value| !value.trim().is_empty()) else {
        return true;
    };
    let Some(record_repo) = record
        .repo_root
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    same_filesystem_path(item_repo, record_repo)
}

fn queue_task_record_launcher_matches_item(
    item: &state::QueueItemRow,
    record: &state::TaskRecordRow,
) -> bool {
    item.remote_launcher.as_deref() == task_record_remote_launcher(record).as_deref()
}

fn task_record_remote_launcher(record: &state::TaskRecordRow) -> Option<String> {
    task_record_metadata_object(record)
        .and_then(|metadata| {
            metadata
                .get("remote_launcher")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|value| !value.trim().is_empty())
}

fn queue_task_record_is_terminal(record: &state::TaskRecordRow) -> bool {
    record.status.starts_with("closed")
}

fn task_record_scope_summary(record: &state::TaskRecordRow) -> String {
    record
        .repo_root
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map_or_else(|| record.id.clone(), |repo| format!("{} in {repo}", record.id))
}

fn same_filesystem_path(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let left = PathBuf::from(left);
    let right = PathBuf::from(right);
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn sync_remote_queue_task_records(item: &state::QueueItemRow, force: bool) -> Result<()> {
    let Some(launcher) = item.remote_launcher.as_deref() else {
        return Ok(());
    };
    if !force && !remote_queue_sync_due(item, launcher) {
        return Ok(());
    }
    let repo_root = queue_item_repo_root(item)?;
    let repo_root_arg = repo_root.display().to_string();
    let output = Command::new(queue_qcold_executable()?)
        .current_dir(&repo_root)
        .env("QCOLD_REPO_ROOT", &repo_root)
        .args([
            "task-record",
            "sync-remote",
            "--via",
            launcher,
            "--local-repo-root",
            &repo_root_arg,
        ])
        .output()
        .with_context(|| format!("failed to sync remote task records through {launcher}"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "remote task-record sync through {launcher} failed with {}: {}",
            output.status,
            compact_process_output(&stdout, &stderr)
        );
    }
    Ok(())
}

fn remote_queue_sync_due(item: &state::QueueItemRow, launcher: &str) -> bool {
    const REMOTE_QUEUE_SYNC_INTERVAL_SECS: u64 = 15;
    let repo = item.repo_root.as_deref().unwrap_or("");
    let key = format!("{launcher}\t{repo}");
    let now = unix_now();
    let sync_times = WEB_QUEUE_REMOTE_SYNC_AT.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut sync_times) = sync_times.lock() else {
        return true;
    };
    if sync_times
        .get(&key)
        .is_some_and(|last| now.saturating_sub(*last) < REMOTE_QUEUE_SYNC_INTERVAL_SECS)
    {
        return false;
    }
    sync_times.insert(key, now);
    true
}

fn compact_process_output(stdout: &str, stderr: &str) -> String {
    let mut output = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if output.len() > QUEUE_PROCESS_OUTPUT_LIMIT {
        output.truncate(QUEUE_PROCESS_OUTPUT_LIMIT);
        output.push_str("...");
    }
    output
}
