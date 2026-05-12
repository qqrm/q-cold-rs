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
    send_terminal_key(&target, TerminalKey::Enter)?;
    Ok(true)
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
