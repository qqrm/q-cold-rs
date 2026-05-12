use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

use crate::webapp;

const DEFAULT_LISTEN: &str = "127.0.0.1:8787";
const DEFAULT_SERVICE_NAME: &str = "qcold-dashboard";

#[derive(Args)]
pub struct WslArgs {
    #[command(subcommand)]
    command: WslSubcommand,
}

#[derive(Subcommand)]
enum WslSubcommand {
    #[command(about = "Manage WSL boot autostart for the local Q-COLD dashboard")]
    Autostart(AutostartArgs),
}

#[derive(Args)]
struct AutostartArgs {
    #[command(subcommand)]
    command: AutostartCommand,
}

#[derive(Subcommand)]
enum AutostartCommand {
    #[command(about = "Install and optionally start the WSL user systemd service")]
    Install(AutostartInstallArgs),
    #[command(about = "Show WSL autostart service state")]
    Status(AutostartServiceArgs),
    #[command(about = "Disable and remove the WSL autostart service")]
    Remove(AutostartServiceArgs),
}

#[derive(Args)]
struct AutostartInstallArgs {
    #[arg(long, default_value = DEFAULT_LISTEN)]
    listen: String,
    #[arg(long, default_value = DEFAULT_SERVICE_NAME)]
    service_name: String,
    #[arg(long, help = "Repository root used as the dashboard daemon working directory")]
    repo_root: Option<PathBuf>,
    #[arg(long, help = "Q-COLD executable path written into the systemd unit")]
    qcold_bin: Option<PathBuf>,
    #[arg(long, help = "Enable the systemd unit without starting it immediately")]
    no_start: bool,
    #[arg(long, help = "Write the user service even when this process is not running inside WSL")]
    force: bool,
}

#[derive(Args)]
struct AutostartServiceArgs {
    #[arg(long, default_value = DEFAULT_SERVICE_NAME)]
    service_name: String,
}

struct UnitSpec {
    description: String,
    qcold_bin: PathBuf,
    repo_root: PathBuf,
    listen: String,
    path: Option<String>,
    state_dir: Option<String>,
}

pub fn run(args: WslArgs) -> Result<u8> {
    match args.command {
        WslSubcommand::Autostart(args) => autostart(args),
    }
}

fn autostart(args: AutostartArgs) -> Result<u8> {
    match args.command {
        AutostartCommand::Install(args) => install_autostart(args),
        AutostartCommand::Status(args) => status_autostart(&args),
        AutostartCommand::Remove(args) => remove_autostart(&args),
    }
}

fn install_autostart(args: AutostartInstallArgs) -> Result<u8> {
    if !args.force && !is_wsl() {
        bail!("WSL autostart install must run inside WSL; pass --force to write the user unit anyway");
    }
    let systemd_state = require_user_systemd()?;
    let unit_name = normalize_service_name(&args.service_name)?;
    let unit_path = systemd_user_unit_path(&unit_name)?;
    let repo_root = existing_absolute_dir(args.repo_root)?;
    let qcold_bin = executable_path(args.qcold_bin)?;
    let spec = UnitSpec {
        description: "Q-COLD local dashboard".to_string(),
        qcold_bin,
        repo_root,
        listen: args.listen.clone(),
        path: optional_env("PATH"),
        state_dir: optional_env("QCOLD_STATE_DIR"),
    };
    let unit = render_unit(&spec);

    if let Some(parent) = unit_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&unit_path, unit)
        .with_context(|| format!("failed to write {}", unit_path.display()))?;

    run_systemctl(&["daemon-reload"])?;
    if !args.no_start {
        webapp::stop_daemon_for_listen(&args.listen)?;
    }
    if args.no_start {
        run_systemctl(&["enable", &unit_name])?;
    } else {
        run_systemctl(&["enable", "--now", &unit_name])?;
    }

    println!("wsl-autostart\tinstalled");
    println!("systemd_user\t{systemd_state}");
    println!("service\t{unit_name}");
    println!("unit\t{}", unit_path.display());
    println!("listen\thttp://{}", args.listen);
    if args.no_start {
        println!("start\tpending");
    } else {
        println!("active\t{}", systemctl_value(&["is-active", &unit_name]));
    }
    Ok(0)
}

fn status_autostart(args: &AutostartServiceArgs) -> Result<u8> {
    let unit_name = normalize_service_name(&args.service_name)?;
    let unit_path = systemd_user_unit_path(&unit_name)?;
    println!("wsl\t{}", yes_no(is_wsl()));
    println!("systemd_user\t{}", user_systemd_status());
    println!("service\t{unit_name}");
    println!("unit\t{}", unit_path.display());
    println!("unit_file\t{}", yes_no(unit_path.is_file()));
    println!("enabled\t{}", systemctl_value(&["is-enabled", &unit_name]));
    println!("active\t{}", systemctl_value(&["is-active", &unit_name]));
    Ok(0)
}

fn remove_autostart(args: &AutostartServiceArgs) -> Result<u8> {
    let unit_name = normalize_service_name(&args.service_name)?;
    let unit_path = systemd_user_unit_path(&unit_name)?;
    let _ = Command::new("systemctl")
        .arg("--user")
        .arg("disable")
        .arg("--now")
        .arg(&unit_name)
        .output();
    if unit_path.exists() {
        fs::remove_file(&unit_path)
            .with_context(|| format!("failed to remove {}", unit_path.display()))?;
    }
    run_systemctl(&["daemon-reload"])?;
    println!("wsl-autostart\tremoved");
    println!("service\t{unit_name}");
    println!("unit\t{}", unit_path.display());
    Ok(0)
}

fn require_user_systemd() -> Result<String> {
    let status = user_systemd_status();
    if matches!(status.as_str(), "running" | "degraded") {
        return Ok(status);
    }
    bail!(
        "WSL user systemd is not available ({status}). Enable systemd in /etc/wsl.conf, \
         run `wsl.exe --shutdown` from Windows, then retry."
    );
}

fn user_systemd_status() -> String {
    systemctl_value(&["is-system-running"])
}

fn run_systemctl(args: &[&str]) -> Result<String> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .with_context(|| format!("failed to run systemctl --user {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "systemctl --user {} failed with {}\nstdout: {}\nstderr: {}",
            args.join(" "),
            output.status,
            trim_output(&output.stdout),
            trim_output(&output.stderr)
        );
    }
    Ok(trim_output(&output.stdout))
}

fn systemctl_value(args: &[&str]) -> String {
    let Ok(output) = Command::new("systemctl").arg("--user").args(args).output() else {
        return "unavailable".to_string();
    };
    let value = trim_output(&output.stdout);
    if output.status.success() {
        return value;
    }
    if !value.is_empty() {
        return value;
    }
    let stderr = trim_output(&output.stderr);
    if stderr.is_empty() {
        "unknown".to_string()
    } else {
        stderr
    }
}

fn trim_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

fn systemd_user_unit_path(unit_name: &str) -> Result<PathBuf> {
    Ok(config_dir()?.join("systemd").join("user").join(unit_name))
}

fn config_dir() -> Result<PathBuf> {
    if let Some(path) = optional_env("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path));
    }
    let home = env::var("HOME").context("HOME is required when XDG_CONFIG_HOME is unset")?;
    Ok(PathBuf::from(home).join(".config"))
}

fn existing_absolute_dir(path: Option<PathBuf>) -> Result<PathBuf> {
    let path = match path {
        Some(path) => path,
        None => env::current_dir().context("failed to resolve current directory")?,
    };
    let path = absolute_path(&path)?;
    if !path.is_dir() {
        bail!("repository root is not a directory: {}", path.display());
    }
    Ok(path)
}

fn executable_path(path: Option<PathBuf>) -> Result<PathBuf> {
    let path = match path {
        Some(path) => path,
        None => find_on_path("qcold")
            .or_else(|| env::current_exe().ok())
            .context("failed to find qcold on PATH or resolve current executable")?,
    };
    let path = absolute_path(&path)?;
    if !is_executable_file(&path) {
        bail!("Q-COLD executable is not runnable: {}", path.display());
    }
    Ok(path)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(env::current_dir()
        .context("failed to resolve current directory")?
        .join(path))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
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

fn normalize_service_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("service name cannot be empty");
    }
    if name.contains('/') || name.contains('\\') || name.chars().any(char::is_whitespace) {
        bail!("service name must not contain path separators or whitespace: {name}");
    }
    if name.ends_with(".service") {
        Ok(name.to_string())
    } else {
        Ok(format!("{name}.service"))
    }
}

fn render_unit(spec: &UnitSpec) -> String {
    let mut lines = vec![
        "[Unit]".to_string(),
        format!("Description={}", spec.description),
        "After=network-online.target".to_string(),
        String::new(),
        "[Service]".to_string(),
        "Type=simple".to_string(),
        format!(
            "WorkingDirectory={}",
            systemd_path(&path_text(&spec.repo_root))
        ),
    ];
    if let Some(path) = spec.path.as_deref() {
        lines.push(format!("Environment={}", systemd_quote(&format!("PATH={path}"))));
    }
    if let Some(state_dir) = spec.state_dir.as_deref() {
        lines.push(format!(
            "Environment={}",
            systemd_quote(&format!("QCOLD_STATE_DIR={state_dir}"))
        ));
    }
    lines.push(format!(
        "ExecStart={}",
        exec_start_line(&spec.qcold_bin, &spec.listen)
    ));
    lines.extend([
        "Restart=on-failure".to_string(),
        "RestartSec=2".to_string(),
        String::new(),
        "[Install]".to_string(),
        "WantedBy=default.target".to_string(),
    ]);
    format!("{}\n", lines.join("\n"))
}

fn exec_start_line(qcold_bin: &Path, listen: &str) -> String {
    [
        systemd_quote(&path_text(qcold_bin)),
        systemd_quote("telegram"),
        systemd_quote("serve"),
        systemd_quote("--listen"),
        systemd_quote(listen),
    ]
    .join(" ")
}

fn systemd_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '%' => quoted.push_str("%%"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

fn systemd_path(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '%' => escaped.push_str("%%"),
            ' ' => escaped.push_str("\\x20"),
            '\t' => escaped.push_str("\\x09"),
            '\n' => escaped.push_str("\\x0a"),
            '"' => escaped.push_str("\\x22"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn path_text(path: &Path) -> String {
    path.as_os_str().to_string_lossy().to_string()
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn is_wsl() -> bool {
    read_os_text("/proc/sys/kernel/osrelease").is_some_and(|value| looks_like_wsl(&value))
        || read_os_text("/proc/version").is_some_and(|value| looks_like_wsl(&value))
}

fn read_os_text(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn looks_like_wsl(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("microsoft") || value.contains("wsl")
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_defaults_to_systemd_unit_suffix() {
        assert_eq!(
            normalize_service_name("qcold-dashboard").unwrap(),
            "qcold-dashboard.service"
        );
        assert_eq!(
            normalize_service_name("qcold-dashboard.service").unwrap(),
            "qcold-dashboard.service"
        );
        assert!(normalize_service_name("bad/name").is_err());
        assert!(normalize_service_name("bad name").is_err());
    }

    #[test]
    fn wsl_detection_accepts_microsoft_kernel_markers() {
        assert!(looks_like_wsl("6.6.114.1-microsoft-standard-WSL2"));
        assert!(looks_like_wsl("Linux version 5.15.90.1-microsoft-standard-WSL2"));
        assert!(!looks_like_wsl("6.8.0-31-generic"));
    }

    #[test]
    fn systemd_unit_runs_qcold_dashboard_in_foreground() {
        let spec = UnitSpec {
            description: "Q-COLD local dashboard".to_string(),
            qcold_bin: PathBuf::from("/home/me/.cargo/bin/qcold"),
            repo_root: PathBuf::from("/home/me/repos/qcold"),
            listen: "127.0.0.1:8787".to_string(),
            path: Some("/home/me/.cargo/bin:/usr/bin".to_string()),
            state_dir: Some("/home/me/.local/state/qcold".to_string()),
        };

        let unit = render_unit(&spec);

        assert!(unit.contains("WorkingDirectory=/home/me/repos/qcold"));
        assert!(unit.contains("Environment=\"PATH=/home/me/.cargo/bin:/usr/bin\""));
        assert!(unit.contains("Environment=\"QCOLD_STATE_DIR=/home/me/.local/state/qcold\""));
        assert!(
            unit.contains(
                "ExecStart=\"/home/me/.cargo/bin/qcold\" \"telegram\" \"serve\" \
                 \"--listen\" \"127.0.0.1:8787\""
            )
        );
        assert!(!unit.contains("--daemon"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn systemd_quote_escapes_unit_special_characters() {
        assert_eq!(systemd_quote(r#"/tmp/a "b" 100%\x"#), r#""/tmp/a \"b\" 100%%\\x""#);
    }

    #[test]
    fn systemd_path_is_unquoted_and_escaped_for_path_directives() {
        assert_eq!(
            systemd_path(r#"/tmp/a "b" 100%\x"#),
            r#"/tmp/a\x20\x22b\x22\x20100%%\\x"#
        );
    }
}
