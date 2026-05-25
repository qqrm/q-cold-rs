#[cfg(test)]
fn queue_task_instruction(item: &state::QueueItemRow) -> String {
    queue_task_instruction_inner(item, item.remote_launcher.as_deref(), None)
}

fn queue_task_instruction_with_task(item: &state::QueueItemRow, task: &QueueManagedTask) -> String {
    queue_task_instruction_inner(
        item,
        task.remote_launcher.as_deref(),
        task.remote_worktree.as_deref(),
    )
}

fn queue_task_instruction_inner(
    item: &state::QueueItemRow,
    remote_launcher: Option<&str>,
    remote_worktree: Option<&str>,
) -> String {
    let root = item.repo_root.as_deref().unwrap_or("<repo>");
    let prompt_snippet = prompt::prompt_snippet(&item.prompt);
    let mut packet = String::new();
    let _ = writeln!(packet, "Q-COLD_TASK_PACKET");
    let _ = writeln!(packet, "repo_root: {root}");
    let _ = writeln!(packet, "task_slug: {}", item.slug);
    let _ = writeln!(packet, "selected_command: {}", item.agent_command);
    write_queue_launch_context(&mut packet, remote_launcher, remote_worktree);
    let _ = writeln!(packet, "required_flow:");
    write_queue_required_flow(&mut packet, remote_launcher.is_some());
    let _ = writeln!(packet, "state_pointers:");
    let _ = writeln!(packet, "  task_env: .task/task.env");
    let _ = writeln!(packet, "  task_logs: .task/logs/");
    let _ = writeln!(packet, "validation_closeout:");
    let _ = writeln!(packet, "  expect: run relevant validation, then terminal closeout");
    let _ = writeln!(
        packet,
        "  success: qcold task closeout --outcome success --message \"<message>\""
    );
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
) {
    if let Some(launcher) = remote_launcher {
        let _ = writeln!(packet, "remote_launcher: {launcher}");
        if let Some(worktree) = remote_worktree {
            let _ = writeln!(packet, "remote_task_worktree: {worktree}");
        }
        let _ = writeln!(
            packet,
            "launch_context: backend-opened remote managed task worktree"
        );
    } else {
        let _ = writeln!(packet, "launch_context: backend-opened managed task worktree");
    }
}

fn write_queue_required_flow(packet: &mut String, remote: bool) {
    if remote {
        let _ = writeln!(
            packet,
            "  - do not open a local task; Q-COLD already opened the remote task"
        );
        let _ = writeln!(packet, "  - keep this Codex executor local for auth, VPN, and chat access");
        let _ = writeln!(packet, "  - run repository commands through the remote launcher");
        let _ = writeln!(packet, "  - use remote_task_worktree as the remote cwd for repository work");
    } else {
        let _ = writeln!(packet, "  - do not run qcold task open; Q-COLD already opened it");
        let _ = writeln!(packet, "  - confirm pwd contains .task/task.env");
    }
    let _ = writeln!(packet, "  - reread AGENTS.md and available task logs");
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

fn queue_agent_launch_command(item: &state::QueueItemRow, _task: &QueueManagedTask) -> String {
    item.agent_command.clone()
}
