fn verify_command(args: &[OsString]) -> Result<u8> {
    if !args.is_empty() {
        println!("verify-profile\t{}", display_args(args));
    }
    let profile = PreflightProfile::parse(args)?;
    run_preflight(profile)?;
    Ok(0)
}

#[derive(Clone, Copy, Default)]
struct PreflightProfile {
    full: bool,
    task_flow: bool,
}

impl PreflightProfile {
    fn parse(args: &[OsString]) -> Result<Self> {
        let mut profile = Self::default();
        for arg in args {
            match arg.to_string_lossy().as_ref() {
                "fast" | "default" => {}
                "full" | "--full" => profile.full = true,
                "task-flow" | "--task-flow" => profile.task_flow = true,
                "-h" | "--help" => {
                    bail!("usage: cargo xtask verify [fast|full|task-flow] [--full] [--task-flow]")
                }
                value => bail!("unknown verify profile argument: {value}"),
            }
        }
        Ok(profile)
    }
}

fn run_preflight(profile: PreflightProfile) -> Result<()> {
    quality::run(&repo_root()?)?;
    run_required("cargo", ["fmt", "--check"].map(OsString::from).to_vec())?;
    run_web_asset_syntax_check()?;
    run_required(
        "cargo",
        ["test", "--locked", "-p", "xtask"]
            .map(OsString::from)
            .to_vec(),
    )?;
    run_required(
        "cargo",
        ["test", "--locked", "--bins"].map(OsString::from).to_vec(),
    )?;
    run_required(
        "cargo",
        ["test", "--locked", "--test", "command_version"]
            .map(OsString::from)
            .to_vec(),
    )?;
    run_required(
        "cargo",
        ["test", "--locked", "--test", "agent_repo_context"]
            .map(OsString::from)
            .to_vec(),
    )?;
    run_required(
        "cargo",
        ["test", "--locked", "--test", "task_record_sequence"]
            .map(OsString::from)
            .to_vec(),
    )?;
    run_required(
        "cargo",
        ["test", "--locked", "--test", "task_flow_record_sync"]
            .map(OsString::from)
            .to_vec(),
    )?;
    run_required(
        "cargo",
        [
            "clippy",
            "--locked",
            "--workspace",
            "--bins",
            "--",
            "-D",
            "clippy::correctness",
            "-D",
            "clippy::suspicious",
            "-D",
            "clippy::perf",
        ]
        .map(OsString::from)
        .to_vec(),
    )?;

    if profile.full {
        run_required("cargo", ["test", "--locked"].map(OsString::from).to_vec())?;
    }
    if profile.task_flow {
        run_required(
            "cargo",
            ["test", "--locked", "--test", "task_flow_control_plane"]
                .map(OsString::from)
                .to_vec(),
        )?;
        run_required(
            "cargo",
            ["test", "--locked", "--test", "task_flow_regression"]
                .map(OsString::from)
                .to_vec(),
        )?;
    }
    Ok(())
}

fn run_web_asset_syntax_check() -> Result<()> {
    let repo = repo_root()?;
    let mut script = String::new();
    for asset in [
        "src/webapp_assets/app/init_parse.js",
        "src/webapp_assets/app/queue.js",
        "src/webapp_assets/app/terminal.js",
        "src/webapp_assets/app/events.js",
    ] {
        script.push_str(
            &fs::read_to_string(repo.join(asset))
                .with_context(|| format!("failed to read {asset}"))?,
        );
        script.push('\n');
    }
    let temp_path = std::env::temp_dir().join(format!(
        "qcold-webapp-asset-check-{}-{}.js",
        std::process::id(),
        unix_now()
    ));
    fs::write(&temp_path, script)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    let result = run_required(
        "node",
        ["--check", path_arg(&temp_path)]
            .map(OsString::from)
            .to_vec(),
    );
    fs::remove_file(&temp_path).ok();
    result
}

fn install_command(args: &[OsString]) -> Result<u8> {
    let mut cargo_args = vec![
        OsString::from("install"),
        OsString::from("--path"),
        OsString::from("."),
        OsString::from("--locked"),
    ];
    cargo_args.extend(args.iter().cloned());
    run_status("cargo", cargo_args)
}

fn not_applicable(kind: &str, args: &[OsString]) -> u8 {
    println!("{kind}\tnot-applicable\t{}", display_args(args));
    0
}

fn cargo_args(command: &str, extra: &[OsString]) -> Vec<OsString> {
    let mut args = vec![OsString::from(command), OsString::from("--locked")];
    args.extend(extra.iter().cloned());
    args
}
