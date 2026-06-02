#[cfg(test)]
fn queue_task_instruction(item: &state::QueueItemRow) -> String {
    queue_task_instruction_inner(item, item.remote_launcher.as_deref(), None, false)
}

fn queue_task_instruction_with_task(
    item: &state::QueueItemRow,
    task: &QueueLaunchWorkspace,
) -> String {
    queue_task_instruction_inner(
        item,
        task.remote_launcher.as_deref(),
        task.remote_worktree.as_deref(),
        task.existing_task,
    )
}

fn queue_remote_native_task_instruction(item: &state::QueueItemRow) -> String {
    queue_task_instruction_inner(item, item.remote_launcher.as_deref(), None, true)
}

fn queue_task_instruction_inner(
    item: &state::QueueItemRow,
    remote_launcher: Option<&str>,
    remote_worktree: Option<&str>,
    existing_task: bool,
) -> String {
    let root = item.repo_root.as_deref().unwrap_or("<repo>");
    let prompt_snippet = prompt::prompt_snippet(&item.prompt);
    let mut packet = String::new();
    let _ = writeln!(packet, "Q-COLD_TASK_PACKET");
    let _ = writeln!(packet, "repo_root: {root}");
    let _ = writeln!(packet, "task_slug: {}", item.slug);
    let _ = writeln!(packet, "execution_host: {}", item.execution_host);
    let _ = writeln!(packet, "selected_command: {}", item.agent_command);
    write_queue_launch_context(&mut packet, remote_launcher, remote_worktree, existing_task);
    let _ = writeln!(packet, "required_flow:");
    write_queue_required_flow(
        &mut packet,
        QueueRequiredFlowContext {
            has_remote_launcher: remote_launcher.is_some(),
            has_remote_worktree: remote_worktree.is_some(),
            existing_task,
            remote_native: item.execution_host == "remote-native",
        },
    );
    write_queue_auto_recovery(&mut packet, item);
    let _ = writeln!(packet, "state_pointers:");
    write_queue_state_pointers(&mut packet, remote_worktree.is_some(), existing_task);
    let _ = writeln!(packet, "validation_closeout:");
    write_queue_validation_closeout(&mut packet, remote_worktree.is_some());
    let _ = writeln!(packet, "blocker_boundary:");
    let _ = writeln!(packet, "  pause_or_blocked_only_for: business decision or external resource");
    let _ = writeln!(packet, "output_guard:");
    write_queue_output_guard_policy(&mut packet);
    let _ = writeln!(packet, "operator_request_snippet: |");
    for line in prompt_snippet.lines() {
        let _ = writeln!(packet, "  {line}");
    }
    let _ = writeln!(packet, "operator_request: |");
    for line in item.prompt.trim().lines() {
        let _ = writeln!(packet, "  {line}");
    }
    let _ = writeln!(packet, "after_closeout: stop; the queue backend owns the executor lifecycle");
    let _ = writeln!(packet, "END_Q-COLD_TASK_PACKET");
    packet
}

fn write_queue_launch_context(
    packet: &mut String,
    remote_launcher: Option<&str>,
    remote_worktree: Option<&str>,
    existing_task: bool,
) {
    if let Some(launcher) = remote_launcher {
        let _ = writeln!(packet, "available_remote_launcher: {launcher}");
    }
    if let Some(worktree) = remote_worktree {
        let _ = writeln!(packet, "remote_task_worktree: {worktree}");
        let _ = writeln!(packet, "launch_context: existing remote managed task record");
    } else if existing_task {
        let _ = writeln!(packet, "launch_context: existing managed task worktree");
    } else {
        let _ = writeln!(packet, "launch_context: executor-owned task environment selection");
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "packet rendering combines independent launch facts without storing long-lived state"
)]
#[derive(Clone, Copy)]
struct QueueRequiredFlowContext {
    has_remote_launcher: bool,
    has_remote_worktree: bool,
    existing_task: bool,
    remote_native: bool,
}

fn write_queue_required_flow(packet: &mut String, context: QueueRequiredFlowContext) {
    if context.remote_native {
        let _ = writeln!(
            packet,
            "  - Q-COLD launched this executor through the repository remote-agent contract"
        );
        let _ = writeln!(
            packet,
            "  - continue in the current remote managed task worktree and do not reopen task_slug"
        );
    } else if context.has_remote_worktree {
        let _ = writeln!(
            packet,
            "  - an existing remote task record was found; re-enter it if it still matches the goal"
        );
        let _ = writeln!(packet, "  - use remote_task_worktree as the remote cwd for repository work");
        let _ = writeln!(
            packet,
            "  - Q-COLD did not choose a new profile, container, or proof environment for this run"
        );
        let _ = writeln!(
            packet,
            "  - if a different repo-approved environment is required, make that choice inside the task"
        );
    } else if context.existing_task {
        let _ = writeln!(
            packet,
            "  - an existing managed task worktree was found; continue it if it still matches the goal"
        );
        let _ = writeln!(
            packet,
            "  - Q-COLD did not choose a new profile, container, or proof environment for this run"
        );
    } else {
        let _ = writeln!(
            packet,
            "  - Q-COLD has not opened task_slug and has not selected a profile or container"
        );
        let _ = writeln!(
            packet,
            "  - start or resume the repository-managed task-flow for task_slug from repo_root"
        );
    }
    let _ = writeln!(
        packet,
        "  - choose the required repo-approved environment from AGENTS.md and the operator request"
    );
    if context.has_remote_launcher && context.remote_native {
        let _ = writeln!(
            packet,
            "  - available_remote_launcher records the local launcher Q-COLD used to reach this remote session"
        );
    } else if context.has_remote_launcher {
        let _ = writeln!(
            packet,
            "  - available_remote_launcher is a convenience launcher, not a selected profile"
        );
    }
    if context.remote_native {
        let _ = writeln!(packet, "  - this Codex executor chat is running on the remote host");
    } else {
        let _ = writeln!(
            packet,
            "  - keep this Codex executor chat local; run substantive work where the repo flow requires"
        );
    }
    let _ = writeln!(
        packet,
        "  - reread root AGENTS.md, nearest AGENTS.md, and task logs after entering the actual task worktree"
    );
    let _ = writeln!(
        packet,
        "  - make task/<task_slug> visible to local Q-COLD; sync remote task records if remote closeout is used"
    );
}

fn write_queue_auto_recovery(packet: &mut String, item: &state::QueueItemRow) {
    if item.recovery_attempts == 0 {
        return;
    }
    let _ = writeln!(packet, "auto_recovery:");
    let _ = writeln!(
        packet,
        "  attempt: {}/{}",
        item.recovery_attempts, WEB_QUEUE_AUTO_RECOVERY_ATTEMPTS
    );
    let _ = writeln!(
        packet,
        "  - inspect the failed task record, .task logs, and flow problems before changing code"
    );
    let _ = writeln!(
        packet,
        "  - make one repair attempt; do not start an unbounded retry loop"
    );
    let _ = writeln!(
        packet,
        "  - if the repair works, close task_slug successfully so the queue can continue"
    );
    let _ = writeln!(
        packet,
        "  - if it still fails, stop after the terminal non-success closeout"
    );
    if !item.message.trim().is_empty() {
        let _ = writeln!(packet, "  previous_failure: |");
        for line in item.message.trim().lines() {
            let _ = writeln!(packet, "    {line}");
        }
    }
}

fn write_queue_validation_closeout(packet: &mut String, remote_worktree: bool) {
    if remote_worktree {
        let _ = writeln!(
            packet,
            "  expect: run relevant validation, pass pre-merge review, then terminal closeout"
        );
        let _ = writeln!(
            packet,
            "  success: use the repository task-flow closeout surface, then sync local Q-COLD if needed"
        );
    } else {
        let _ = writeln!(
            packet,
            "  expect: run relevant validation, pass pre-merge review, then terminal closeout"
        );
        let _ = writeln!(
            packet,
            "  success: qcold task closeout --outcome success --message \"<message>\""
        );
    }
}

fn write_queue_state_pointers(packet: &mut String, remote_worktree: bool, existing_task: bool) {
    if remote_worktree {
        let _ = writeln!(packet, "  task_env: remote_task_worktree/.task/task.env");
        let _ = writeln!(packet, "  task_logs: remote_task_worktree/.task/logs/");
    } else if existing_task {
        let _ = writeln!(packet, "  task_env: .task/task.env");
        let _ = writeln!(packet, "  task_logs: .task/logs/");
    } else {
        let _ = writeln!(packet, "  task_env: <actual-task-worktree>/.task/task.env");
        let _ = writeln!(packet, "  task_logs: <actual-task-worktree>/.task/logs/");
    }
}

fn write_queue_output_guard_policy(packet: &mut String) {
    let _ = writeln!(
        packet,
        "  - shape broad searches first with rg -l, rg --count, wc, git diff --stat, or head/tail"
    );
    let _ = writeln!(
        packet,
        "  - Q-COLD-started agents automatically guard configured broad-output commands"
    );
    let _ = writeln!(
        packet,
        "  - if a command reports qcold-guard status=blocked, rerun a narrower command"
    );
    let _ = writeln!(
        packet,
        "  - if output is blocked or too large, rerun a narrower query or inspect a focused slice"
    );
}

fn write_queue_task_packet_file(
    item: &state::QueueItemRow,
    task: &QueueLaunchWorkspace,
) -> Result<PathBuf> {
    write_queue_task_packet_text_file(item, &queue_task_instruction_with_task(item, task))
}

fn write_remote_native_task_packet_file(item: &state::QueueItemRow) -> Result<PathBuf> {
    write_queue_task_packet_text_file(item, &queue_remote_native_task_instruction(item))
}

fn write_queue_task_packet_text_file(
    item: &state::QueueItemRow,
    packet: &str,
) -> Result<PathBuf> {
    let directory = state::state_dir()?.join("queue-task-packets");
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let path = directory.join(queue_task_packet_file_name(item));
    fs::write(&path, packet)
        .with_context(|| format!("failed to write queue task packet {}", path.display()))?;
    Ok(path)
}

fn queue_task_packet_file_name(item: &state::QueueItemRow) -> String {
    let base = sanitize_daemon_id(&format!("{}-{}", item.run_id, item.id));
    let base = if base.is_empty() {
        "queue-task".to_string()
    } else {
        base.chars().take(80).collect()
    };
    format!("{}-{}.prompt", base, stable_short_hash(&format!("{}:{}", item.run_id, item.id)))
}

fn cleanup_queue_task_packet_file(path: &Path) {
    let _ = fs::remove_file(path);
}

fn queue_agent_launch_command(
    item: &state::QueueItemRow,
    task: &QueueLaunchWorkspace,
    prompt_file: &Path,
) -> String {
    format!(
        "{} exec --dangerously-bypass-approvals-and-sandbox -C {} - < {}",
        queue_shell_quote(&item.agent_command),
        queue_shell_quote(&task.worktree.display().to_string()),
        queue_shell_quote(&prompt_file.display().to_string())
    )
}
