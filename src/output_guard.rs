use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

pub(crate) const DEFAULT_AGENT_OUTPUT_GUARD_COMMANDS: &str = "rg,grep,find,cat,git,unzip,zcat,jq";

const OUTPUT_GUARD_DISABLE_ENV: &str = "QCOLD_AGENT_OUTPUT_GUARD";
const OUTPUT_GUARD_COMMANDS_ENV: &str = "QCOLD_AGENT_OUTPUT_GUARD_COMMANDS";
const OUTPUT_GUARD_ENABLED_ENV: &str = "QCOLD_OUTPUT_GUARD_ENABLED";
const OUTPUT_GUARD_BIN_ENV: &str = "QCOLD_OUTPUT_GUARD_BIN";
const OUTPUT_GUARD_COMMAND_LIST_ENV: &str = "QCOLD_OUTPUT_GUARD_COMMANDS";
const OUTPUT_GUARD_QCOLD_ENV: &str = "QCOLD_GUARD_QCOLD";
const OUTPUT_GUARD_REAL_PREFIX: &str = "QCOLD_GUARD_REAL_";

#[derive(Clone, Debug)]
pub(crate) struct OutputGuardLaunch {
    pub(crate) bin_dir: PathBuf,
    pub(crate) qcold_path: PathBuf,
    pub(crate) real_commands: Vec<GuardedCommand>,
}

#[derive(Clone, Debug)]
pub(crate) struct GuardedCommand {
    pub(crate) command: String,
    pub(crate) env_name: String,
    pub(crate) real_path: PathBuf,
}

pub(crate) fn prepare_output_guard_launch(
    id: &str,
    started_at: u64,
) -> Result<Option<OutputGuardLaunch>> {
    if output_guard_disabled() {
        return Ok(None);
    }
    let commands = output_guard_commands()?;
    let path_dirs = env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let inherited_guard_bin = env::var_os(OUTPUT_GUARD_BIN_ENV).map(PathBuf::from);
    prepare_output_guard_launch_with_paths(
        id,
        started_at,
        commands,
        &path_dirs,
        inherited_guard_bin.as_deref(),
    )
}

pub(crate) fn prepare_output_guard_launch_with_paths(
    id: &str,
    started_at: u64,
    commands: Vec<String>,
    path_dirs: &[PathBuf],
    inherited_guard_bin: Option<&Path>,
) -> Result<Option<OutputGuardLaunch>> {
    if commands.is_empty() {
        return Ok(None);
    }
    let mut real_commands = Vec::new();
    for (index, command) in commands.into_iter().enumerate() {
        validate_guard_command_name(&command)?;
        let Some(real_path) =
            command_path_skipping_guard_dirs(&command, path_dirs, inherited_guard_bin)
        else {
            continue;
        };
        real_commands.push(GuardedCommand {
            env_name: guard_real_env_name(index, &command),
            command,
            real_path,
        });
    }
    if real_commands.is_empty() {
        return Ok(None);
    }

    let bin_dir = crate::state::state_dir()?
        .join("guard-bin")
        .join(format!("{}-{started_at}", sanitize_id(id)));
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create output guard bin {}", bin_dir.display()))?;
    for guarded in &real_commands {
        write_output_guard_wrapper(&bin_dir, guarded)?;
    }
    Ok(Some(OutputGuardLaunch {
        bin_dir,
        qcold_path: env::current_exe().context("failed to resolve current qcold executable")?,
        real_commands,
    }))
}

pub(crate) fn apply_output_guard_to_command(
    command: &mut Command,
    output_guard: Option<&OutputGuardLaunch>,
) {
    scrub_inherited_output_guard(command);
    if let Some(output_guard) = output_guard {
        command.env(OUTPUT_GUARD_ENABLED_ENV, "yes");
        command.env(OUTPUT_GUARD_BIN_ENV, &output_guard.bin_dir);
        command.env(
            OUTPUT_GUARD_COMMAND_LIST_ENV,
            guarded_command_list(output_guard),
        );
        command.env(OUTPUT_GUARD_QCOLD_ENV, &output_guard.qcold_path);
        for guarded in &output_guard.real_commands {
            command.env(&guarded.env_name, &guarded.real_path);
        }
        let path = env::var("PATH").unwrap_or_default();
        let inherited_guard_bin = env::var_os(OUTPUT_GUARD_BIN_ENV).map(PathBuf::from);
        let cleaned_path = path_without_output_guard_bin(&path, inherited_guard_bin.as_deref());
        command.env(
            "PATH",
            guarded_path_value(output_guard.bin_dir.as_path(), &cleaned_path),
        );
    }
}

pub(crate) fn apply_output_guard_metadata_to_command(
    command: &mut Command,
    output_guard: Option<&OutputGuardLaunch>,
) {
    command.env(
        OUTPUT_GUARD_ENABLED_ENV,
        if output_guard.is_some() { "yes" } else { "no" },
    );
    if let Some(output_guard) = output_guard {
        command.env(OUTPUT_GUARD_BIN_ENV, &output_guard.bin_dir);
        command.env(
            OUTPUT_GUARD_COMMAND_LIST_ENV,
            guarded_command_list(output_guard),
        );
        command.env(OUTPUT_GUARD_QCOLD_ENV, &output_guard.qcold_path);
        for guarded in &output_guard.real_commands {
            command.env(&guarded.env_name, &guarded.real_path);
        }
    }
}

pub(crate) fn scrub_inherited_output_guard(command: &mut Command) {
    command.env_remove(OUTPUT_GUARD_ENABLED_ENV);
    command.env_remove(OUTPUT_GUARD_BIN_ENV);
    command.env_remove(OUTPUT_GUARD_COMMAND_LIST_ENV);
    command.env_remove(OUTPUT_GUARD_QCOLD_ENV);
    for (key, _) in env::vars_os() {
        if key
            .to_str()
            .is_some_and(|name| name.starts_with(OUTPUT_GUARD_REAL_PREFIX))
        {
            command.env_remove(key);
        }
    }
    let Some(inherited_guard_bin) = env::var_os(OUTPUT_GUARD_BIN_ENV).map(PathBuf::from) else {
        return;
    };
    let path = env::var("PATH").unwrap_or_default();
    let cleaned_path = path_without_output_guard_bin(&path, Some(inherited_guard_bin.as_path()));
    if cleaned_path != path {
        command.env("PATH", cleaned_path);
    }
}

pub(crate) fn terminal_output_guard_env_prefix_with_path(
    output_guard: Option<&OutputGuardLaunch>,
    path: Option<&str>,
) -> String {
    let mut prefix = String::new();
    let inherited_guard_bin = env::var_os(OUTPUT_GUARD_BIN_ENV).map(PathBuf::from);
    if inherited_guard_bin.is_some() || output_guard.is_some() {
        prefix.push_str(
            "unset QCOLD_OUTPUT_GUARD_ENABLED QCOLD_OUTPUT_GUARD_BIN \
             QCOLD_OUTPUT_GUARD_COMMANDS QCOLD_GUARD_QCOLD; ",
        );
        for (key, _) in env::vars() {
            if key.starts_with(OUTPUT_GUARD_REAL_PREFIX) {
                prefix.push_str("unset ");
                prefix.push_str(&key);
                prefix.push_str("; ");
            }
        }
    }
    let path = path.unwrap_or_default();
    let cleaned_path = path_without_output_guard_bin(path, inherited_guard_bin.as_deref());
    if let Some(output_guard) = output_guard {
        use std::fmt::Write as _;

        let _ = write!(prefix, "export QCOLD_OUTPUT_GUARD_ENABLED=yes; ");
        let _ = write!(
            prefix,
            "export QCOLD_OUTPUT_GUARD_BIN={}; ",
            shell_quote(&output_guard.bin_dir.display().to_string())
        );
        let _ = write!(
            prefix,
            "export QCOLD_OUTPUT_GUARD_COMMANDS={}; ",
            shell_quote(&guarded_command_list(output_guard))
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
        let guarded_path = guarded_path_value(output_guard.bin_dir.as_path(), &cleaned_path);
        let _ = write!(prefix, "export PATH={}; ", shell_quote(&guarded_path));
    } else if inherited_guard_bin.is_some() && cleaned_path != path {
        use std::fmt::Write as _;

        let _ = write!(prefix, "export PATH={}; ", shell_quote(&cleaned_path));
    }
    prefix
}

pub(crate) fn guarded_path_value(guard_bin: &Path, path: &str) -> String {
    if path.is_empty() {
        guard_bin.display().to_string()
    } else {
        format!("{}:{path}", guard_bin.display())
    }
}

pub(crate) fn path_without_output_guard_bin(
    path: &str,
    inherited_guard_bin: Option<&Path>,
) -> String {
    let Some(inherited_guard_bin) = inherited_guard_bin else {
        return path.to_string();
    };
    let dirs = env::split_paths(path)
        .filter(|dir| dir.as_path() != inherited_guard_bin)
        .collect::<Vec<_>>();
    env::join_paths(dirs)
        .map(|paths| paths.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub(crate) fn write_output_guard_wrapper(bin_dir: &Path, guarded: &GuardedCommand) -> Result<()> {
    let wrapper = bin_dir.join(&guarded.command);
    let script = format!(
        "#!/bin/sh\nexec \"$QCOLD_GUARD_QCOLD\" guard -- \"${}\" \"$@\"\n",
        guarded.env_name
    );
    fs::write(&wrapper, script)
        .with_context(|| format!("failed to write output guard wrapper {}", wrapper.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&wrapper)
            .with_context(|| format!("failed to stat output guard wrapper {}", wrapper.display()))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).with_context(|| {
            format!(
                "failed to make output guard wrapper executable {}",
                wrapper.display()
            )
        })?;
    }
    Ok(())
}

pub(crate) fn output_guard_commands() -> Result<Vec<String>> {
    let raw = env::var(OUTPUT_GUARD_COMMANDS_ENV)
        .unwrap_or_else(|_| DEFAULT_AGENT_OUTPUT_GUARD_COMMANDS.to_string());
    parse_output_guard_commands(&raw)
}

pub(crate) fn parse_output_guard_commands(raw: &str) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut commands = Vec::new();
    for command in raw.split(',').map(str::trim) {
        if command.is_empty() {
            continue;
        }
        validate_guard_command_name(command)?;
        if seen.insert(command.to_string()) {
            commands.push(command.to_string());
        }
    }
    Ok(commands)
}

pub(crate) fn guarded_command_list(output_guard: &OutputGuardLaunch) -> String {
    output_guard
        .real_commands
        .iter()
        .map(|guarded| guarded.command.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn output_guard_disabled() -> bool {
    env::var(OUTPUT_GUARD_DISABLE_ENV).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

fn command_path_skipping_guard_dirs(
    command: &str,
    path_dirs: &[PathBuf],
    inherited_guard_bin: Option<&Path>,
) -> Option<PathBuf> {
    path_dirs
        .iter()
        .filter(|dir| inherited_guard_bin != Some(dir.as_path()))
        .map(|dir| dir.join(command))
        .find(|candidate| executable_file(candidate))
}

fn validate_guard_command_name(command: &str) -> Result<()> {
    if !command.is_empty()
        && command
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Ok(());
    }
    bail!(
        "invalid QCOLD_AGENT_OUTPUT_GUARD_COMMANDS entry {command:?}; use bare command names \
         containing only ASCII letters, digits, '-' or '_'"
    )
}

fn guard_real_env_name(index: usize, command: &str) -> String {
    format!(
        "QCOLD_GUARD_REAL_{index}_{}",
        command
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
    )
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}
