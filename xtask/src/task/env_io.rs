#[derive(Default)]
struct TaskEnv {
    task_id: String,
    task_name: String,
    task_sequence: String,
    task_branch: String,
    task_execution_anchor: String,
    task_description: String,
    task_worktree: PathBuf,
    task_profile: String,
    primary_repo_path: PathBuf,
    base_branch: String,
    base_head: String,
    task_head: String,
    started_at: String,
    status: String,
    updated_at: String,
    devcontainer_name: String,
    delivery_mode: String,
    codex_thread_id: String,
    codex_rollout_path: String,
    output_guard_enabled: String,
    output_guard_bin: String,
    output_guard_commands: String,
    output_guard_qcold: String,
    output_guard_real_commands: Vec<(String, String)>,
}

fn current_task_env() -> Result<TaskEnv> {
    let root = repo_root()?;
    let env_path = root.join(".task/task.env");
    if !env_path.is_file() {
        bail!("run this from a managed task worktree");
    }
    parse_task_env(&env_path)
}

fn open_tasks(repo: &Path) -> Result<Vec<TaskEnv>> {
    let root = managed_root(repo)?;
    let mut tasks = Vec::new();
    if !root.exists() {
        return Ok(tasks);
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let env_path = entry.path().join(".task/task.env");
        if env_path.is_file() {
            tasks.push(parse_task_env(&env_path)?);
        }
    }
    tasks.sort_by(|left, right| left.task_name.cmp(&right.task_name));
    Ok(tasks)
}

fn task_blocks_terminal(status: &str) -> bool {
    status.is_empty() || status == "open" || status == "paused" || status == "failed-closeout"
}

fn find_task(repo: &Path, task_slug: &str) -> Result<Option<TaskEnv>> {
    Ok(open_tasks(repo)?.into_iter().find(|task| {
        task.task_name == task_slug
            || task.task_branch == format!("task/{task_slug}")
            || task
                .task_worktree
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(task_slug))
    }))
}

fn parse_task_env(path: &Path) -> Result<TaskEnv> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let value = |key: &str| {
        content
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(unquote)
            .unwrap_or_default()
    };
    let output_guard_real_commands = content
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _raw)| key.starts_with("QCOLD_GUARD_REAL_"))
        .map(|(key, raw)| (key.to_string(), unquote(raw)))
        .collect();
    Ok(TaskEnv {
        task_id: value("TASK_ID"),
        task_name: value("TASK_NAME"),
        task_sequence: value("TASK_SEQUENCE"),
        task_branch: value("TASK_BRANCH"),
        task_execution_anchor: value("TASK_EXECUTION_ANCHOR"),
        task_description: value("TASK_DESCRIPTION"),
        task_worktree: PathBuf::from(value("TASK_WORKTREE")),
        task_profile: value("TASK_PROFILE"),
        primary_repo_path: PathBuf::from(value("PRIMARY_REPO_PATH")),
        base_branch: value("BASE_BRANCH"),
        base_head: value("BASE_HEAD"),
        task_head: value("TASK_HEAD"),
        started_at: value("STARTED_AT"),
        status: value("STATUS"),
        updated_at: value("UPDATED_AT"),
        devcontainer_name: value("DEVCONTAINER_NAME"),
        delivery_mode: value("DELIVERY_MODE"),
        codex_thread_id: value("CODEX_THREAD_ID"),
        codex_rollout_path: value("CODEX_ROLLOUT_PATH"),
        output_guard_enabled: value("QCOLD_OUTPUT_GUARD_ENABLED"),
        output_guard_bin: value("QCOLD_OUTPUT_GUARD_BIN"),
        output_guard_commands: value("QCOLD_OUTPUT_GUARD_COMMANDS"),
        output_guard_qcold: value("QCOLD_GUARD_QCOLD"),
        output_guard_real_commands,
    })
}

fn write_task_env(task: &TaskEnv) -> Result<()> {
    let dir = task.task_worktree.join(".task/logs");
    fs::create_dir_all(&dir)?;
    let env_path = task.task_worktree.join(".task/task.env");
    let fields = [
        ("TASK_ID", task.task_id.as_str()),
        ("TASK_NAME", task.task_name.as_str()),
        ("TASK_SEQUENCE", task.task_sequence.as_str()),
        ("TASK_BRANCH", task.task_branch.as_str()),
        ("TASK_EXECUTION_ANCHOR", task.task_execution_anchor.as_str()),
        ("TASK_DESCRIPTION", task.task_description.as_str()),
        ("TASK_WORKTREE", &task.task_worktree.display().to_string()),
        ("TASK_PROFILE", task.task_profile.as_str()),
        (
            "PRIMARY_REPO_PATH",
            &task.primary_repo_path.display().to_string(),
        ),
        ("BASE_BRANCH", task.base_branch.as_str()),
        ("BASE_HEAD", task.base_head.as_str()),
        ("TASK_HEAD", task.task_head.as_str()),
        ("STARTED_AT", task.started_at.as_str()),
        ("STATUS", task.status.as_str()),
        ("UPDATED_AT", task.updated_at.as_str()),
        ("DEVCONTAINER_NAME", task.devcontainer_name.as_str()),
        ("DELIVERY_MODE", task.delivery_mode.as_str()),
    ];
    let mut output = String::new();
    for (key, value) in fields {
        output.push_str(key);
        output.push('=');
        output.push_str(&shell_quote(value));
        output.push('\n');
    }
    for (key, value) in [
        ("CODEX_THREAD_ID", task.codex_thread_id.as_str()),
        ("CODEX_ROLLOUT_PATH", task.codex_rollout_path.as_str()),
        (
            "QCOLD_OUTPUT_GUARD_ENABLED",
            task.output_guard_enabled.as_str(),
        ),
        ("QCOLD_OUTPUT_GUARD_BIN", task.output_guard_bin.as_str()),
        (
            "QCOLD_OUTPUT_GUARD_COMMANDS",
            task.output_guard_commands.as_str(),
        ),
        ("QCOLD_GUARD_QCOLD", task.output_guard_qcold.as_str()),
    ] {
        if value.is_empty() {
            continue;
        }
        output.push_str(key);
        output.push('=');
        output.push_str(&shell_quote(value));
        output.push('\n');
    }
    let mut real_commands = task.output_guard_real_commands.clone();
    real_commands.sort_by(|left, right| left.0.cmp(&right.0));
    for (key, value) in real_commands {
        if value.is_empty() {
            continue;
        }
        output.push_str(&key);
        output.push('=');
        output.push_str(&shell_quote(&value));
        output.push('\n');
    }
    fs::write(env_path, output)?;
    Ok(())
}

fn refresh_task_codex_env(task: &mut TaskEnv) {
    if let Some(thread_id) = nonempty_env("CODEX_THREAD_ID") {
        task.codex_thread_id = thread_id;
    }
    let thread_id = nonempty_str(&task.codex_thread_id);
    if let Some(rollout_path) = crate::rollout::current_codex_rollout_path(thread_id) {
        task.codex_rollout_path = rollout_path.display().to_string();
    }
}

fn refresh_task_output_guard_env(task: &mut TaskEnv) {
    task.output_guard_enabled = nonempty_env("QCOLD_OUTPUT_GUARD_ENABLED")
        .unwrap_or_else(|| "no".to_string());
    task.output_guard_bin = nonempty_env("QCOLD_OUTPUT_GUARD_BIN").unwrap_or_default();
    task.output_guard_commands =
        nonempty_env("QCOLD_OUTPUT_GUARD_COMMANDS").unwrap_or_default();
    task.output_guard_qcold = nonempty_env("QCOLD_GUARD_QCOLD").unwrap_or_default();
    let mut real_commands = std::env::vars()
        .filter(|(key, value)| {
            key.starts_with("QCOLD_GUARD_REAL_") && !value.trim().is_empty()
        })
        .collect::<Vec<_>>();
    real_commands.sort_by(|left, right| left.0.cmp(&right.0));
    task.output_guard_real_commands = real_commands;
}

fn task_output_guard_shell_exports(task: &TaskEnv) -> String {
    if task.output_guard_enabled != "yes" || task.output_guard_bin.trim().is_empty() {
        return String::new();
    }
    let mut output = String::new();
    output.push_str("export QCOLD_OUTPUT_GUARD_ENABLED=yes\n");
    for (key, value) in [
        ("QCOLD_OUTPUT_GUARD_BIN", task.output_guard_bin.as_str()),
        (
            "QCOLD_OUTPUT_GUARD_COMMANDS",
            task.output_guard_commands.as_str(),
        ),
        ("QCOLD_GUARD_QCOLD", task.output_guard_qcold.as_str()),
    ] {
        if value.trim().is_empty() {
            continue;
        }
        output.push_str("export ");
        output.push_str(key);
        output.push('=');
        output.push_str(&shell_quote(value));
        output.push('\n');
    }
    let mut real_commands = task.output_guard_real_commands.clone();
    real_commands.sort_by(|left, right| left.0.cmp(&right.0));
    for (key, value) in real_commands {
        if value.trim().is_empty() {
            continue;
        }
        output.push_str("export ");
        output.push_str(&key);
        output.push('=');
        output.push_str(&shell_quote(&value));
        output.push('\n');
    }
    output.push_str("case \":$PATH:\" in *:");
    output.push_str(&shell_quote(&task.output_guard_bin));
    output.push_str(":*) ;; *) export PATH=");
    output.push_str(&shell_quote(&task.output_guard_bin));
    output.push_str(":\"$PATH\"");
    output.push_str(" ;; esac\n");
    output
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn nonempty_str(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn append_event(worktree: &Path, kind: &str, message: &str) -> Result<()> {
    let path = worktree.join(".task/logs/events.ndjson");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let event = format!(
        "{{\"kind\":\"{}\",\"message\":\"{}\",\"timestamp\":{}}}\n",
        json_escape(kind),
        json_escape(message),
        unix_now()
    );
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all_ext(event.as_bytes())
}

trait WriteAllExt {
    fn write_all_ext(self, bytes: &[u8]) -> Result<()>;
}

impl WriteAllExt for fs::File {
    fn write_all_ext(mut self, bytes: &[u8]) -> Result<()> {
        use std::io::Write as _;

        self.write_all(bytes)?;
        Ok(())
    }
}

fn repo_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to locate git root")?;
    if !output.status.success() {
        bail!("not inside a git checkout");
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string()).canonicalize()?)
}

fn task_inventory_repo_root() -> Result<PathBuf> {
    let root = repo_root()?;
    let env_path = root.join(".task/task.env");
    if !env_path.is_file() {
        return Ok(root);
    }
    let task = parse_task_env(&env_path)?;
    if task.primary_repo_path.as_os_str().is_empty() {
        return Ok(root);
    }
    task.primary_repo_path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", task.primary_repo_path.display()))
}

fn managed_root(repo: &Path) -> Result<PathBuf> {
    Ok(repo
        .parent()
        .context("repository root has no parent")?
        .join("WT")
        .join(repo.file_name().context("repository root has no name")?))
}

fn ensure_clean(repo: &Path, label: &str) -> Result<()> {
    let status = git_output(repo, ["status", "--porcelain"])?;
    if status.is_empty() {
        Ok(())
    } else {
        bail!("{label} is dirty:\n{status}")
    }
}

fn ensure_slug(slug: &str) -> Result<()> {
    if slug.is_empty()
        || slug.starts_with('-')
        || !slug
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        bail!("task slug must contain only ASCII letters, digits, '-' or '_'");
    }
    Ok(())
}

fn run_git<const N: usize>(repo: &Path, args: [&str; N]) -> Result<()> {
    let display = args.join(" ");
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .with_context(|| format!("failed to run git {display}"))?;
    if !status.success() {
        bail!("git command failed: git {display}");
    }
    Ok(())
}

fn git_status<const N: usize>(repo: &Path, args: [&str; N]) -> Result<bool> {
    let display = args.join(" ");
    Ok(Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .with_context(|| format!("failed to run git {display}"))?
        .success())
}

fn git_output<const N: usize>(repo: &Path, args: [&str; N]) -> Result<String> {
    let display = args.join(" ");
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {display}"))?;
    if !output.status.success() {
        bail!("git command failed: git {display}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_status(program: &str, args: Vec<OsString>) -> Result<u8> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {program}"))?;
    Ok(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1))
}

fn run_required(program: &str, args: Vec<OsString>) -> Result<()> {
    let code = run_status(program, args)?;
    if code == 0 {
        Ok(())
    } else {
        bail!("{program} validation failed with code {code}");
    }
}

fn qcold_task_sequence() -> Option<u64> {
    std::env::var("QCOLD_TASK_SEQUENCE")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
}

fn sequence_anchor(sequence: u64) -> Option<String> {
    (sequence > 0).then(|| format!("{sequence:03}"))
}

fn short_anchor() -> String {
    format!("{:x}", unix_now())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn path_arg(path: &Path) -> &str {
    path.to_str().unwrap_or("")
}

fn display_args(args: &[OsString]) -> String {
    args.iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    if value.contains(['\n', '\r']) {
        return format!("$'{}'", ansi_c_escape(value));
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "-_./:".contains(ch))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 3 && value.starts_with("$'") && value.ends_with('\'') {
        return ansi_c_unescape(&value[2..value.len() - 1]);
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("'\\''", "'")
    } else {
        value.to_string()
    }
}

fn ansi_c_escape(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\'"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn ansi_c_unescape(value: &str) -> String {
    let mut unescaped = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            unescaped.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') | None => unescaped.push('\\'),
            Some('\'') => unescaped.push('\''),
            Some('n') => unescaped.push('\n'),
            Some('r') => unescaped.push('\r'),
            Some('t') => unescaped.push('\t'),
            Some(other) => {
                unescaped.push('\\');
                unescaped.push(other);
            }
        }
    }
    unescaped
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
