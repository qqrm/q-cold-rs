const REVIEWER_COMMAND_ENV: &str = "QCOLD_CLOSEOUT_REVIEWER_COMMAND";
const PRE_MERGE_REVIEW_PATH: &str = ".task/review/pre-merge-review.md";
const PRE_MERGE_REVIEW_ENV_PATH: &str = ".task/review/pre-merge-review.env";
const PRE_MERGE_REVIEW_PROMPT_PATH: &str = ".task/review/pre-merge-review-prompt.md";
const PRE_MERGE_REVIEW_COMMAND_LOG_PATH: &str = ".task/review/pre-merge-review-command.log";
const BUNDLE_PRE_MERGE_REVIEW_REPORT: &str = "evidence/pre-merge-review.md";
const BUNDLE_PRE_MERGE_REVIEW_ENV: &str = "metadata/pre-merge-review.env";

fn run_pre_merge_review(task: &TaskEnv) -> Result<()> {
    let started_at = unix_now();
    let before = review_target_fingerprint(task)?;
    let report_path = task.task_worktree.join(PRE_MERGE_REVIEW_PATH);
    let metadata_path = task.task_worktree.join(PRE_MERGE_REVIEW_ENV_PATH);
    let prompt_path = task.task_worktree.join(PRE_MERGE_REVIEW_PROMPT_PATH);
    let command_log_path = task.task_worktree.join(PRE_MERGE_REVIEW_COMMAND_LOG_PATH);
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let prompt = render_pre_merge_review_prompt(task, &before);
    fs::write(&prompt_path, &prompt)?;
    fs::write(&report_path, "")?;

    let (reviewer, output) = if let Some(command) = nonempty_env(REVIEWER_COMMAND_ENV) {
        run_injected_reviewer_command(task, &command, &prompt_path, &report_path)?
    } else {
        run_default_reviewer_command(task, &prompt, &report_path)?
    };
    let finished_at = unix_now();
    fs::write(&command_log_path, render_reviewer_command_log(&output))?;
    let after = review_target_fingerprint(task)?;
    if before.fingerprint != after.fingerprint {
        bail!(
            "pre-merge reviewer changed the review target; before={} after={}",
            before.fingerprint,
            after.fingerprint
        );
    }
    if !output.status.success() {
        bail!("pre-merge reviewer command failed with status {}", output.status);
    }

    let mut report = fs::read_to_string(&report_path).unwrap_or_default();
    if report.trim().is_empty() {
        report = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if report.is_empty() {
            report = String::from_utf8_lossy(&output.stderr).trim().to_string();
        }
        fs::write(&report_path, &report)?;
    }
    let verdict = parse_pre_merge_review_report(&report)?;
    let metadata = PreMergeReviewMetadata {
        status: verdict.status.to_string(),
        summary: verdict.summary,
        reviewer,
        report: BUNDLE_PRE_MERGE_REVIEW_REPORT.to_string(),
        metadata: BUNDLE_PRE_MERGE_REVIEW_ENV.to_string(),
        started_at,
        finished_at,
        task_head: before.head,
        fingerprint: before.fingerprint,
        finding_count: verdict.finding_count,
    };
    fs::write(&metadata_path, render_pre_merge_review_metadata(&metadata))?;
    if metadata.status == "block" {
        append_event(
            &task.task_worktree,
            "task-closeout-review",
            &format!("pre-merge-review blocked: {}", metadata.summary),
        )?;
        bail!("pre-merge reviewer blocked integration; see {PRE_MERGE_REVIEW_PATH}");
    }
    append_event(
        &task.task_worktree,
        "task-closeout-review",
        &format!("pre-merge-review passed: {}", metadata.summary),
    )?;
    println!("task-closeout-review\tpassed\t{}", report_path.display());
    println!("PRE_MERGE_REVIEW_SUMMARY={}", metadata.summary);
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct PreMergeReviewMetadata {
    status: String,
    summary: String,
    reviewer: String,
    report: String,
    metadata: String,
    started_at: u64,
    finished_at: u64,
    task_head: String,
    fingerprint: String,
    finding_count: usize,
}

fn read_pre_merge_review_metadata(task: &TaskEnv) -> Option<PreMergeReviewMetadata> {
    let path = task.task_worktree.join(PRE_MERGE_REVIEW_ENV_PATH);
    let content = fs::read_to_string(path).ok()?;
    Some(parse_pre_merge_review_metadata(&content))
}

fn parse_pre_merge_review_metadata(content: &str) -> PreMergeReviewMetadata {
    let value = |key: &str| {
        content
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(unquote)
            .unwrap_or_default()
    };
    PreMergeReviewMetadata {
        status: value("PRE_MERGE_REVIEW_STATUS"),
        summary: value("PRE_MERGE_REVIEW_SUMMARY"),
        reviewer: value("PRE_MERGE_REVIEW_REVIEWER"),
        report: value("PRE_MERGE_REVIEW_REPORT"),
        metadata: value("PRE_MERGE_REVIEW_METADATA"),
        started_at: value("PRE_MERGE_REVIEW_STARTED_AT").parse().unwrap_or_default(),
        finished_at: value("PRE_MERGE_REVIEW_FINISHED_AT")
            .parse()
            .unwrap_or_default(),
        task_head: value("PRE_MERGE_REVIEW_TASK_HEAD"),
        fingerprint: value("PRE_MERGE_REVIEW_FINGERPRINT"),
        finding_count: value("PRE_MERGE_REVIEW_FINDING_COUNT")
            .parse()
            .unwrap_or_default(),
    }
}

fn render_pre_merge_review_metadata(metadata: &PreMergeReviewMetadata) -> String {
    let mut output = String::new();
    for (key, value) in [
        ("PRE_MERGE_REVIEW_STATUS", metadata.status.clone()),
        ("PRE_MERGE_REVIEW_SUMMARY", metadata.summary.clone()),
        ("PRE_MERGE_REVIEW_REVIEWER", metadata.reviewer.clone()),
        ("PRE_MERGE_REVIEW_REPORT", metadata.report.clone()),
        ("PRE_MERGE_REVIEW_METADATA", metadata.metadata.clone()),
        (
            "PRE_MERGE_REVIEW_STARTED_AT",
            metadata.started_at.to_string(),
        ),
        (
            "PRE_MERGE_REVIEW_FINISHED_AT",
            metadata.finished_at.to_string(),
        ),
        ("PRE_MERGE_REVIEW_TASK_HEAD", metadata.task_head.clone()),
        (
            "PRE_MERGE_REVIEW_FINGERPRINT",
            metadata.fingerprint.clone(),
        ),
        (
            "PRE_MERGE_REVIEW_FINDING_COUNT",
            metadata.finding_count.to_string(),
        ),
    ] {
        output.push_str(key);
        output.push('=');
        output.push_str(&shell_quote(&value));
        output.push('\n');
    }
    output
}

fn run_injected_reviewer_command(
    task: &TaskEnv,
    command: &str,
    prompt_path: &Path,
    report_path: &Path,
) -> Result<(String, std::process::Output)> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(&task.task_worktree)
        .env("QCOLD_REVIEW_TASK_WORKTREE", &task.task_worktree)
        .env("QCOLD_REVIEW_PRIMARY_REPO", &task.primary_repo_path)
        .env("QCOLD_REVIEW_PROMPT", prompt_path)
        .env("QCOLD_REVIEW_OUTPUT", report_path)
        .env("QCOLD_REVIEW_BASE_HEAD", &task.base_head)
        .env("QCOLD_REVIEW_TASK_HEAD", &task.task_head)
        .output()
        .context("failed to run injected pre-merge reviewer command")?;
    Ok((format!("injected:{REVIEWER_COMMAND_ENV}"), output))
}

fn run_default_reviewer_command(
    task: &TaskEnv,
    prompt: &str,
    report_path: &Path,
) -> Result<(String, std::process::Output)> {
    let mut child = Command::new("c1")
        .args([
            "exec",
            "-C",
            path_arg(&task.task_worktree),
            "--sandbox",
            "read-only",
            "--output-last-message",
            path_arg(report_path),
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to launch default c1 pre-merge reviewer")?;
    child
        .stdin
        .as_mut()
        .context("failed to open reviewer stdin")?
        .write_all(prompt.as_bytes())
        .context("failed to write pre-merge reviewer prompt")?;
    let output = child
        .wait_with_output()
        .context("failed to wait for default c1 pre-merge reviewer")?;
    Ok(("c1 exec quality_auditor".to_string(), output))
}

struct ReviewTargetFingerprint {
    head: String,
    fingerprint: String,
    status_short: String,
    changed_files: String,
    diff_stat: String,
    staged_diff_stat: String,
}

fn review_target_fingerprint(task: &TaskEnv) -> Result<ReviewTargetFingerprint> {
    let head = git_output(&task.task_worktree, ["rev-parse", "HEAD"])?;
    let status_short = review_target_status_short(task)?;
    let changed_files = status_short.clone();
    let diff_stat = git_output(&task.task_worktree, ["diff", "--stat"]).unwrap_or_default();
    let staged_diff_stat =
        git_output(&task.task_worktree, ["diff", "--cached", "--stat"]).unwrap_or_default();
    let diff = git_output(&task.task_worktree, ["diff", "--binary"]).unwrap_or_default();
    let staged_diff =
        git_output(&task.task_worktree, ["diff", "--cached", "--binary"]).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&head, &mut hasher);
    std::hash::Hash::hash(&status_short, &mut hasher);
    std::hash::Hash::hash(&diff, &mut hasher);
    std::hash::Hash::hash(&staged_diff, &mut hasher);
    hash_untracked_review_target_files(task, &mut hasher)?;
    let fingerprint = format!("{:016x}", std::hash::Hasher::finish(&hasher));
    Ok(ReviewTargetFingerprint {
        head,
        fingerprint,
        status_short,
        changed_files,
        diff_stat,
        staged_diff_stat,
    })
}

fn hash_untracked_review_target_files(
    task: &TaskEnv,
    hasher: &mut std::collections::hash_map::DefaultHasher,
) -> Result<()> {
    let output = Command::new("git")
        .current_dir(&task.task_worktree)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()
        .context("failed to inspect untracked review target files")?;
    if !output.status.success() {
        bail!("git ls-files failed with status {}", output.status);
    }
    let mut paths = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .filter(|path| !Path::new(path).starts_with(".task"))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let full = task.task_worktree.join(&path);
        let metadata = fs::symlink_metadata(&full)
            .with_context(|| format!("failed to inspect untracked review target {path}"))?;
        std::hash::Hash::hash(&path, hasher);
        if metadata.file_type().is_symlink() {
            std::hash::Hash::hash("symlink", hasher);
            let target = fs::read_link(&full)
                .with_context(|| format!("failed to read untracked symlink {path}"))?;
            std::hash::Hash::hash(&target.to_string_lossy(), hasher);
        } else if metadata.is_file() {
            std::hash::Hash::hash("file", hasher);
            let content = fs::read(&full)
                .with_context(|| format!("failed to read untracked review target {path}"))?;
            std::hash::Hash::hash(&content, hasher);
        } else {
            std::hash::Hash::hash("other", hasher);
        }
    }
    Ok(())
}

fn review_target_status_short(task: &TaskEnv) -> Result<String> {
    let status = git_output(
        &task.task_worktree,
        ["status", "--short", "--untracked-files=all"],
    )?;
    Ok(status
        .lines()
        .filter(|line| !status_path(line).is_some_and(|path| path.starts_with(".task")))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn render_pre_merge_review_prompt(task: &TaskEnv, target: &ReviewTargetFingerprint) -> String {
    format!(
        "You are the quality_auditor specialist for Q-COLD success closeout.\n\
         Review the current task worktree before final integration and push.\n\
         Worktree: {worktree}\n\
         Primary repo: {primary}\n\
         Task: {task_id}\n\
         Branch: {branch}\n\
         Base head: {base_head}\n\
         Task head: {task_head}\n\
         Review fingerprint: {fingerprint}\n\n\
         Scope:\n\
         - Check architecture and adapter boundaries.\n\
         - Check for hacks, brittle shortcuts, and poor engineering practice.\n\
         - Check task fit and whether the patch solves only the requested scope.\n\
         - Check minimal code growth and avoidable complexity.\n\
         - Prefer concrete file/line findings over broad commentary.\n\n\
         Current git status:\n\
         {status}\n\n\
         Changed files:\n\
         {changed_files}\n\n\
         Worktree diff stat:\n\
         {diff_stat}\n\n\
         Staged diff stat:\n\
         {staged_diff_stat}\n\n\
         Do not edit files. Your final report must start with exactly one of:\n\
         REVIEW_STATUS=pass\n\
         REVIEW_STATUS=block\n\
         Add REVIEW_SUMMARY=<one concise sentence> near the top.\n\
         Include at least one bullet beginning '- ' with argued criticism or \
         an argued no-blocking-finding assessment.\n\
         Use block for release-blocking findings that should stop integration.\n",
        worktree = task.task_worktree.display(),
        primary = task.primary_repo_path.display(),
        task_id = task.task_id,
        branch = task.task_branch,
        base_head = task.base_head,
        task_head = target.head,
        fingerprint = target.fingerprint,
        status = empty_marker(&target.status_short),
        changed_files = empty_marker(&target.changed_files),
        diff_stat = empty_marker(&target.diff_stat),
        staged_diff_stat = empty_marker(&target.staged_diff_stat),
    )
}

#[derive(Debug)]
struct PreMergeReviewVerdict {
    status: &'static str,
    summary: String,
    finding_count: usize,
}

fn parse_pre_merge_review_report(report: &str) -> Result<PreMergeReviewVerdict> {
    let status = report
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(|line| line.strip_prefix("REVIEW_STATUS="))
        .context(
            "pre-merge reviewer report must start with REVIEW_STATUS=pass or REVIEW_STATUS=block",
        )?;
    let summary = report
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("REVIEW_SUMMARY="))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .context("pre-merge reviewer report is missing REVIEW_SUMMARY=<summary>")?;
    let finding_count = review_finding_count(report);
    if finding_count == 0 {
        bail!("pre-merge reviewer report has no argued finding bullet");
    }
    match status {
        "pass" => Ok(PreMergeReviewVerdict {
            status: "pass",
            summary,
            finding_count,
        }),
        "block" => Ok(PreMergeReviewVerdict {
            status: "block",
            summary,
            finding_count,
        }),
        other => bail!("unsupported pre-merge reviewer status: {other}"),
    }
}

fn review_finding_count(report: &str) -> usize {
    report
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("- ") && line.len() >= 24)
        .count()
}

fn empty_marker(value: &str) -> &str {
    if value.trim().is_empty() {
        "(none)"
    } else {
        value
    }
}

fn render_reviewer_command_log(output: &std::process::Output) -> String {
    let mut log = String::new();
    log.push_str("STATUS=");
    log.push_str(&output.status.to_string());
    log.push('\n');
    log.push_str("\nSTDOUT\n");
    log.push_str(&String::from_utf8_lossy(&output.stdout));
    log.push_str("\nSTDERR\n");
    log.push_str(&String::from_utf8_lossy(&output.stderr));
    log
}
