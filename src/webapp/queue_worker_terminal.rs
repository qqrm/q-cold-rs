fn agent_terminal_closeout_failed(agent_id: &str) -> bool {
    let Some(output) = agent_terminal_output(agent_id) else {
        return false;
    };
    [
        "Q-COLD closeout could not complete",
        "Could not complete canonical Q-COLD closeout",
        "missing task metadata",
        "repository target mismatch",
        "run this from a managed task worktree",
    ]
    .iter()
    .any(|needle| output.contains(needle))
}

fn agent_terminal_output(agent_id: &str) -> Option<String> {
    let target = agent_terminal_target(agent_id)?;
    capture_agent_terminal_output(&target).ok()
}

fn agent_terminal_target(agent_id: &str) -> Option<String> {
    let context = agents::terminal_contexts()
        .ok()?
        .into_iter()
        .find(|context| context.id == agent_id)?;
    Some(context.target)
}

fn submit_agent_terminal_pending_paste(agent_id: &str) -> Result<bool> {
    let Some(output) = agent_terminal_output(agent_id) else {
        return Ok(false);
    };
    if !terminal_output_has_pending_paste(&output) {
        return Ok(false);
    }
    let Some(target) = agent_terminal_target(agent_id) else {
        return Ok(false);
    };
    send_terminal_submit(&target)?;
    Ok(true)
}

enum QueueTerminalReadiness {
    Ready,
    RestartAfterUpdate,
    Failed,
}

fn wait_for_agent_terminal_ready(agent_id: &str) -> QueueTerminalReadiness {
    let mut accepted_update_prompt = false;
    for _ in 0..240 {
        if let Some(output) = agent_terminal_output(agent_id) {
            if !accepted_update_prompt && terminal_output_has_codex_update_prompt(&output) {
                accepted_update_prompt = true;
                if let Some(target) = agent_terminal_target(agent_id) {
                    let _ = send_terminal_key(&target, TerminalKey::Enter);
                }
            }
            if terminal_output_has_codex_update_restart_notice(&output) {
                return QueueTerminalReadiness::RestartAfterUpdate;
            }
            if terminal_output_ready_for_queue_input(&output) {
                return QueueTerminalReadiness::Ready;
            }
        }
        if !agent_running(agent_id) {
            return QueueTerminalReadiness::Failed;
        }
        thread::sleep(Duration::from_millis(500));
    }
    QueueTerminalReadiness::Failed
}

fn terminal_output_ready_for_queue_input(output: &str) -> bool {
    if terminal_output_has_pending_paste(output) {
        return false;
    }
    let recent = output.lines().rev().take(16).map(str::trim).collect::<Vec<_>>();
    let has_prompt = recent
        .iter()
        .any(|line| terminal_line_has_idle_prompt(line));
    let has_busy_indicator = recent
        .iter()
        .any(|line| terminal_line_has_busy_indicator(line));
    has_prompt && !has_busy_indicator
}

fn terminal_output_has_codex_update_prompt(output: &str) -> bool {
    let recent = output.lines().rev().take(32).map(str::trim).collect::<Vec<_>>();
    let has_update_notice = recent
        .iter()
        .any(|line| line.contains("Update available!"));
    let has_update_action = recent
        .iter()
        .any(|line| line.contains("Update now") && line.contains("@openai/codex"));
    let awaits_enter = recent
        .iter()
        .any(|line| line.contains("Press enter to continue"));
    has_update_notice && has_update_action && awaits_enter
}

fn terminal_output_has_codex_update_restart_notice(output: &str) -> bool {
    let recent = output.lines().rev().take(32).map(str::trim).collect::<Vec<_>>();
    let update_succeeded = recent
        .iter()
        .any(|line| line.contains("Update ran successfully"));
    let restart_required = recent
        .iter()
        .any(|line| line.contains("Please restart Codex"));
    update_succeeded && restart_required
}

fn terminal_line_has_idle_prompt(line: &str) -> bool {
    terminal_line_starts_with_interactive_prompt(line)
        || line.starts_with('>')
        || line.starts_with("gpt-")
        || line.contains(" gpt-")
}

fn terminal_line_starts_with_interactive_prompt(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('›') else {
        return false;
    };
    let rest = rest.trim_start();
    !rest
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit() || ch == '.')
}

fn terminal_line_has_busy_indicator(line: &str) -> bool {
    line.contains("Booting MCP server")
        || line.contains("Working (")
        || line.contains("esc to interrupt")
}

fn terminal_output_has_pending_paste(output: &str) -> bool {
    if output
        .lines()
        .rev()
        .take(12)
        .any(|line| line.contains("[Pasted Content"))
    {
        return true;
    }
    terminal_output_has_unsubmitted_task_packet(output)
}

fn terminal_output_has_unsubmitted_task_packet(output: &str) -> bool {
    let Some((_, after_packet)) = output.rsplit_once("END_Q-COLD_TASK_PACKET") else {
        return false;
    };
    let recent = after_packet.lines().take(12).collect::<Vec<_>>();
    let has_activity = recent
        .iter()
        .any(|line| line.trim_start().starts_with('•'));
    let has_idle_prompt = recent
        .iter()
        .any(|line| line.trim_start().starts_with("gpt-"));
    has_idle_prompt && !has_activity
}

fn capture_agent_terminal_output(target: &str) -> Result<String> {
    if let Some((session, pane)) = parse_zellij_target(target) {
        let output = Command::new("zellij")
            .args([
                "--session",
                session,
                "action",
                "dump-screen",
                "--full",
                "--pane-id",
                pane,
            ])
            .output()
            .with_context(|| format!("failed to dump zellij pane {target}"))?;
        if !output.status.success() {
            bail!("zellij dump-screen failed with {}", output.status);
        }
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let output = Command::new("tmux")
        .args(["capture-pane", "-p", "-t", target, "-S", "-2000"])
        .output()
        .with_context(|| format!("failed to capture tmux pane {target}"))?;
    if !output.status.success() {
        bail!("tmux capture-pane failed with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
