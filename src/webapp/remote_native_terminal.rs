fn send_remote_native_terminal_paste(
    agent_id: &str,
    pane: &str,
    text: &str,
    submit: bool,
) -> Result<()> {
    let item = remote_native_terminal_item(agent_id)?;
    let launcher = item
        .remote_launcher
        .as_deref()
        .context("remote-native queue terminal requires remote launcher")?;
    let buffer = terminal_paste_buffer_name()?;
    let mut child = Command::new(launcher)
        .args(["tmux", "load-buffer", "-b", &buffer, "-"])
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to load remote-native terminal input into tmux buffer")?;
    let mut stdin = child
        .stdin
        .take()
        .context("failed to open remote-native tmux buffer stdin")?;
    stdin
        .write_all(text.as_bytes())
        .context("failed to write remote-native terminal input to tmux buffer")?;
    drop(stdin);
    let status = child
        .wait()
        .context("failed waiting for remote-native tmux load-buffer")?;
    if !status.success() {
        bail!("remote-native tmux load-buffer failed with {status}");
    }

    let target = remote_native_tmux_target(agent_id, pane);
    let mut paste_args = vec!["tmux".to_string()];
    paste_args.extend(tmux_paste_buffer_args(&buffer, &target, true));
    let status = Command::new(launcher)
        .args(paste_args)
        .status()
        .context("failed to paste remote-native terminal input through tmux")?;
    if !status.success() {
        bail!("remote-native tmux paste-buffer failed with {status}");
    }
    if submit {
        thread::sleep(terminal_paste_submit_delay(text));
        send_remote_native_terminal_key(agent_id, pane, TerminalKey::Enter)?;
    }
    Ok(())
}

fn send_remote_native_terminal_literal(
    agent_id: &str,
    pane: &str,
    text: &str,
    submit: bool,
) -> Result<()> {
    let item = remote_native_terminal_item(agent_id)?;
    let launcher = item
        .remote_launcher
        .as_deref()
        .context("remote-native queue terminal requires remote launcher")?;
    let target = remote_native_tmux_target(agent_id, pane);
    let status = Command::new(launcher)
        .args(["tmux", "send-keys", "-t", &target, "-l", text])
        .status()
        .context("failed to send literal remote-native terminal input through tmux")?;
    if !status.success() {
        bail!("remote-native tmux send-keys literal failed with {status}");
    }
    if submit {
        send_remote_native_terminal_key(agent_id, pane, TerminalKey::Enter)?;
    }
    Ok(())
}

fn send_remote_native_terminal_key(agent_id: &str, pane: &str, key: TerminalKey) -> Result<()> {
    let item = remote_native_terminal_item(agent_id)?;
    let launcher = item
        .remote_launcher
        .as_deref()
        .context("remote-native queue terminal requires remote launcher")?;
    let target = remote_native_tmux_target(agent_id, pane);
    let status = Command::new(launcher)
        .args(["tmux", "send-keys", "-t", &target, key.tmux()])
        .status()
        .context("failed to send remote-native terminal key through tmux")?;
    if !status.success() {
        bail!("remote-native tmux send-keys failed with {status}");
    }
    Ok(())
}
