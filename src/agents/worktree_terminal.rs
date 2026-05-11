fn open_agent_worktree(
    id: &str,
    track: &str,
    started_at: u64,
    requested_cwd: &Path,
) -> Result<LaunchContext> {
    let primary_root = git_root_for(requested_cwd)?;
    let relative_cwd = requested_cwd.strip_prefix(&primary_root).unwrap_or(Path::new(""));
    let agent_slug = agent_worktree_slug(id, track, started_at);
    let worktree = agent_worktree_path(&primary_root, &agent_slug)?;
    if worktree.exists() {
        bail!("agent worktree already exists: {}", worktree.display());
    }
    fs::create_dir_all(
        worktree
            .parent()
            .context("agent worktree has no parent")?,
    )?;
    let worktree_arg = worktree.display().to_string();
    let status = Command::new("git")
        .current_dir(&primary_root)
        .args(["worktree", "add", "--detach", &worktree_arg, "HEAD"])
        .status()
        .with_context(|| format!("failed to create agent worktree {agent_slug}"))?;
    if !status.success() {
        bail!("failed to create agent worktree {agent_slug}: {status}");
    }
    ensure_worktree_submodules(&worktree)?;
    let cwd = worktree.join(relative_cwd);
    let cwd = if cwd.is_dir() {
        cwd
    } else {
        worktree.clone()
    };
    Ok(LaunchContext {
        cwd,
        qcold_repo_root: Some(primary_root),
        qcold_agent_worktree: Some(worktree),
    })
}

fn ensure_worktree_submodules(worktree: &Path) -> Result<()> {
    if !worktree.join(".gitmodules").is_file() {
        return Ok(());
    }
    let output = Command::new("git")
        .current_dir(worktree)
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
            "--recursive",
        ])
        .output()
        .with_context(|| format!("failed to initialize submodules in {}", worktree.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "failed to initialize submodules in {}: {}\n{}",
        worktree.display(),
        output.status,
        format_command_output(&stdout, &stderr)
    );
}

fn agent_worktree_slug(id: &str, track: &str, started_at: u64) -> String {
    let id = sanitize_id(id);
    let track = sanitize_id(track);
    let base = if id.is_empty() {
        format!("agent-{track}-{started_at}")
    } else {
        format!("agent-{id}")
    };
    if base == "agent--" || base == "agent-" {
        format!("agent-{started_at}")
    } else {
        base
    }
}

fn agent_worktree_path(primary_root: &Path, agent_slug: &str) -> Result<PathBuf> {
    Ok(primary_root
        .parent()
        .context("repository root has no parent")?
        .join("WT")
        .join(primary_root.file_name().context("repository root has no name")?)
        .join("agents")
        .join(agent_slug))
}

fn format_command_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim(), stderr.trim()) {
        ("", "") => "no output".to_string(),
        (stdout, "") => stdout.to_string(),
        ("", stderr) => stderr.to_string(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    }
}

fn start_tmux_terminal_agent(
    id: &str,
    track: &str,
    started_at: u64,
    launch: &TerminalLaunch,
    stdout_log_path: &Path,
) -> Result<AgentRecord> {
    ensure_tmux_available()?;
    let session = format!("qcold-{id}");
    let target = format!("{session}:0.0");
    let env_prefix = terminal_qcold_env_prefix(
        launch.qcold_repo_root.as_deref(),
        launch.qcold_agent_worktree.as_deref(),
        launch.output_guard.as_ref(),
    );
    let wrapped = format!(
        "{env_prefix}{}; status=$?; printf \
         '\\n[Q-COLD terminal command exited with status %s]\\n' \"$status\"; exit \"$status\"",
        launch.command,
    );
    let delayed = format!("sleep 0.1; exec sh -lc {}", shell_quote(&wrapped));
    let tmux_shell_command = format!("sh -lc {}", shell_quote(&delayed));
    let status = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            &session,
            "-c",
            &launch.cwd.display().to_string(),
            &tmux_shell_command,
        ])
        .status()
        .with_context(|| format!("failed to start tmux session {session}"))?;
    if !status.success() {
        bail!("tmux new-session failed with {status}");
    }
    set_tmux_option(&session, "remain-on-exit", "off")?;
    set_tmux_option(&session, "mouse", "off")?;
    set_tmux_option(&session, "history-limit", "10000")?;
    let pipe_command = format!("cat >> {}", shell_quote(&stdout_log_path.display().to_string()));
    let status = Command::new("tmux")
        .args(["pipe-pane", "-o", "-t", &target, &pipe_command])
        .status()
        .with_context(|| format!("failed to pipe tmux pane {target}"))?;
    if !status.success() {
        bail!("tmux pipe-pane failed with {status}");
    }
    let pid = tmux_pane_pid(&target)?;

    let record = AgentRecord {
        id: id.to_string(),
        track: track.to_string(),
        pid,
        started_at,
        command: vec![
            "tmux".to_string(),
            "new-session".to_string(),
            "-s".to_string(),
            session,
            launch.command.clone(),
        ],
        cwd: Some(launch.cwd.clone()),
    };
    Ok(record)
}

fn start_zellij_terminal_agent(
    id: &str,
    track: &str,
    started_at: u64,
    launch: &TerminalLaunch,
) -> Result<AgentRecord> {
    ensure_zellij_available()?;
    let session = format!("qcold-{id}");
    let env_prefix = terminal_qcold_env_prefix(
        launch.qcold_repo_root.as_deref(),
        launch.qcold_agent_worktree.as_deref(),
        launch.output_guard.as_ref(),
    );
    let wrapped = format!(
        "{env_prefix}{}; status=$?; printf '\\n[Q-COLD terminal command exited with status %s]\\n' \
         \"$status\"; sleep 0.1; zellij delete-session --force {} >/dev/null 2>&1 || true; \
         exit \"$status\"",
        launch.command,
        shell_quote(&session)
    );
    let layout_path = state_dir()?.join("logs").join(format!("{id}.zellij.kdl"));
    fs::write(&layout_path, zellij_layout(id, &wrapped)?)
        .with_context(|| format!("failed to write zellij layout {}", layout_path.display()))?;

    let status = Command::new("zellij")
        .current_dir(&launch.cwd)
        .args([
            "attach",
            "--create-background",
            "--forget",
            &session,
            "options",
            "--default-layout",
            &layout_path.display().to_string(),
            "--mouse-mode",
            "false",
            "--pane-frames",
            "false",
            "--show-release-notes",
            "false",
            "--show-startup-tips",
            "false",
        ])
        .status()
        .with_context(|| format!("failed to create zellij session {session}"))?;
    if !status.success() {
        bail!("zellij attach --create-background failed with {status}");
    }
    let pane = zellij_first_terminal_pane(&session)?;
    let _ = Command::new("zellij")
        .args(["--session", &session, "action", "focus-pane-id", &pane])
        .status();
    let pid = zellij_session_pid(&session)?;

    Ok(AgentRecord {
        id: id.to_string(),
        track: track.to_string(),
        pid,
        started_at,
        command: vec![
            "zellij".to_string(),
            "--session".to_string(),
            session,
            "pane".to_string(),
            pane,
            launch.command.clone(),
        ],
        cwd: Some(launch.cwd.clone()),
    })
}

fn zellij_layout(id: &str, wrapped: &str) -> Result<String> {
    Ok(format!(
        "layout {{\n    pane name={} command=\"sh\" close_on_exit=true {{\n        args \"-lc\" {}\n    }}\n}}\n",
        kdl_quote(id)?,
        kdl_quote(wrapped)?
    ))
}

fn kdl_quote(value: &str) -> Result<String> {
    serde_json::to_string(value).context("failed to quote zellij layout string")
}

fn zellij_first_terminal_pane(session: &str) -> Result<String> {
    let mut last_error = None;
    for _ in 0..20 {
        match zellij_first_terminal_pane_once(session) {
            Ok(pane) => return Ok(pane),
            Err(err) => last_error = Some(err),
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(last_error
        .unwrap_or_else(|| anyhow::anyhow!("zellij session {session} has no terminal pane")))
}

fn apply_qcold_launch_env(
    command: &mut Command,
    root: Option<&Path>,
    agent_worktree: Option<&Path>,
) {
    if let Some(root) = root {
        command.env("QCOLD_REPO_ROOT", root);
    }
    if let Some(agent_worktree) = agent_worktree {
        command.env("QCOLD_AGENT_WORKTREE", agent_worktree);
    }
}

fn terminal_qcold_env_prefix(
    root: Option<&Path>,
    agent_worktree: Option<&Path>,
    output_guard: Option<&OutputGuardLaunch>,
) -> String {
    let path = env::var("PATH").ok();
    terminal_qcold_env_prefix_with_path(root, agent_worktree, output_guard, path.as_deref())
}

fn terminal_qcold_env_prefix_with_path(
    root: Option<&Path>,
    agent_worktree: Option<&Path>,
    output_guard: Option<&OutputGuardLaunch>,
    path: Option<&str>,
) -> String {
    let mut prefix = String::new();
    let inherited_guard_bin = env::var_os("QCOLD_OUTPUT_GUARD_BIN").map(PathBuf::from);
    if let Some(root) = root {
        let _ = write!(
            prefix,
            "export QCOLD_REPO_ROOT={}; ",
            shell_quote(&root.display().to_string())
        );
    }
    if let Some(agent_worktree) = agent_worktree {
        let _ = write!(
            prefix,
            "export QCOLD_AGENT_WORKTREE={}; ",
            shell_quote(&agent_worktree.display().to_string())
        );
    }
    if inherited_guard_bin.is_some() || output_guard.is_some() {
        prefix.push_str("unset QCOLD_OUTPUT_GUARD_BIN QCOLD_GUARD_QCOLD; ");
    }
    let path = path.unwrap_or_default();
    let cleaned_path = path_without_output_guard_bin(path, inherited_guard_bin.as_deref());
    if let Some(output_guard) = output_guard {
        let _ = write!(
            prefix,
            "export QCOLD_OUTPUT_GUARD_BIN={}; ",
            shell_quote(&output_guard.bin_dir.display().to_string())
        );
        let _ = write!(
            prefix,
            "export QCOLD_GUARD_QCOLD={}; ",
            shell_quote(&output_guard.qcold_path.display().to_string())
        );
        for guarded in &output_guard.real_commands {
            let _ = write!(
                prefix,
                "export {}={}; ",
                guarded.env_name,
                shell_quote(&guarded.real_path.display().to_string())
            );
        }
        let guarded_path = if cleaned_path.is_empty() {
            output_guard.bin_dir.display().to_string()
        } else {
            format!("{}:{cleaned_path}", output_guard.bin_dir.display())
        };
        let _ = write!(prefix, "export PATH={}; ", shell_quote(&guarded_path));
    } else if inherited_guard_bin.is_some() && cleaned_path != path {
        let _ = write!(prefix, "export PATH={}; ", shell_quote(&cleaned_path));
    }
    prefix
}

fn path_without_output_guard_bin(path: &str, inherited_guard_bin: Option<&Path>) -> String {
    let Some(inherited_guard_bin) = inherited_guard_bin else {
        return path.to_string();
    };
    let dirs = env::split_paths(path)
        .filter(|dir| dir.as_path() != inherited_guard_bin)
        .collect::<Vec<_>>();
    env::join_paths(dirs)
        .ok()
        .and_then(|path| path.into_string().ok())
        .unwrap_or_else(|| path.to_string())
}

fn zellij_first_terminal_pane_once(session: &str) -> Result<String> {
    let output = Command::new("zellij")
        .args(["--session", session, "action", "list-panes"])
        .output()
        .with_context(|| format!("failed to list zellij panes for session {session}"))?;
    if !output.status.success() {
        bail!("zellij action list-panes failed with {}", output.status);
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .find_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            (fields.len() >= 2 && fields[1] == "terminal").then(|| fields[0].to_string())
        })
        .with_context(|| format!("zellij session {session} has no terminal pane"))
}

fn set_tmux_option(session: &str, name: &str, value: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["set-option", "-t", session, name, value])
        .status()
        .with_context(|| format!("failed to configure tmux session {session}"))?;
    if !status.success() {
        bail!("tmux set-option {name} failed with {status}");
    }
    Ok(())
}

fn ensure_zellij_available() -> Result<()> {
    let status = Command::new("zellij")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("zellij is required for QCOLD_TERMINAL_BACKEND=zellij")?;
    if !status.success() {
        bail!("zellij is required for QCOLD_TERMINAL_BACKEND=zellij");
    }
    Ok(())
}

fn zellij_session_pid(session: &str) -> Result<u32> {
    let marker = format!("/zellij/contract_version_1/{session}");
    let entries = fs::read_dir("/proc").context("failed to inspect /proc for zellij session")?;
    for entry in entries.filter_map(Result::ok) {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args = cmdline
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part));
        if args.into_iter().any(|arg| arg.contains(&marker)) {
            return Ok(pid);
        }
    }
    bail!("failed to locate zellij server process for session {session}");
}

fn ensure_tmux_available() -> Result<()> {
    let status = Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("tmux is required for attachable terminal agents")?;
    if !status.success() {
        bail!("tmux is required for attachable terminal agents");
    }
    Ok(())
}

fn tmux_pane_pid(target: &str) -> Result<u32> {
    let output = Command::new("tmux")
        .args(["display-message", "-p", "-t", target, "#{pane_pid}"])
        .output()
        .with_context(|| format!("failed to read tmux pane pid for {target}"))?;
    if !output.status.success() {
        bail!("tmux display-message failed with {}", output.status);
    }
    let value = String::from_utf8_lossy(&output.stdout);
    value
        .trim()
        .parse()
        .with_context(|| format!("invalid tmux pane pid for {target}: {value}"))
}

fn attach_terminal(record: &AgentRecord) -> Result<()> {
    let target = terminal_target(record).context("agent was not started in a terminal session")?;
    let (program, args, session) = match target {
        TerminalTarget::Tmux { session } => (
            "tmux",
            vec!["attach-session".to_string(), "-t".to_string(), session.clone()],
            session,
        ),
        TerminalTarget::Zellij { session, .. } => {
            ("zellij", vec!["attach".to_string(), session.clone()], session)
        }
    };
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to attach terminal session {session}"))?;
    if !status.success() {
        bail!("terminal attach failed with {status}");
    }
    Ok(())
}

fn terminate_terminal_target(target: &TerminalTarget) -> Result<()> {
    let mut command = match target {
        TerminalTarget::Tmux { session } => {
            let mut command = Command::new("tmux");
            command.args(["kill-session", "-t", session]);
            command
        }
        TerminalTarget::Zellij { session, .. } => {
            let mut command = Command::new("zellij");
            command.args(["delete-session", "--force", session]);
            command
        }
    };
    let status = command.status().context("failed to terminate terminal agent")?;
    if !status.success() {
        bail!("terminal agent termination failed with {status}");
    }
    Ok(())
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> Result<()> {
    let pid = i32::try_from(pid).context("agent pid is too large")?;
    // SAFETY: kill(2) is called with a pid previously recorded by Q-COLD.
    let result = unsafe { libc::kill(pid, libc::SIGTERM) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("kill(SIGTERM) failed");
    }
    Ok(())
}

#[cfg(not(unix))]
fn terminate_process(_pid: u32) -> Result<()> {
    bail!("agent termination is only supported on unix platforms")
}
