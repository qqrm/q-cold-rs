#[derive(Default)]
struct TaskEnv {
    task_id: String,
    task_name: String,
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
    Ok(TaskEnv {
        task_id: value("TASK_ID"),
        task_name: value("TASK_NAME"),
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
    })
}

fn write_task_env(task: &TaskEnv) -> Result<()> {
    let dir = task.task_worktree.join(".task/logs");
    fs::create_dir_all(&dir)?;
    let env_path = task.task_worktree.join(".task/task.env");
    let fields = [
        ("TASK_ID", task.task_id.as_str()),
        ("TASK_NAME", task.task_name.as_str()),
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
    fs::write(env_path, output)?;
    Ok(())
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
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .context("failed to run git")?;
    if !status.success() {
        bail!("git command failed");
    }
    Ok(())
}

fn git_status<const N: usize>(repo: &Path, args: [&str; N]) -> Result<bool> {
    Ok(Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .context("failed to run git")?
        .success())
}

fn git_output<const N: usize>(repo: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .context("failed to run git")?;
    if !output.status.success() {
        bail!("git command failed");
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

fn task_execution_anchor() -> String {
    std::env::var("QCOLD_TASK_SEQUENCE")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(sequence_anchor)
        .unwrap_or_else(short_anchor)
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
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("'\\''", "'")
    } else {
        value.to_string()
    }
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
