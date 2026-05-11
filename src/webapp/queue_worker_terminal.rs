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
    let target = agents::terminal_contexts()
        .ok()?
        .into_iter()
        .find(|context| context.id == agent_id)?
        .target;
    capture_agent_terminal_output(&target).ok()
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
