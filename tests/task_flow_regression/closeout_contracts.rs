use super::*;

pub(crate) fn blocked_failed_and_success_closeout_paths_preserve_terminal_contracts() {
    let fixture = Fixture::new();

    let blocked_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "blocked"])
        .assert()
        .success();
    let blocked_worktree = task_worktree_from_assert(&blocked_open);
    write_file(&blocked_worktree.join("note.txt"), "blocked\n");
    let blocked = fixture
        .run_xtask(
            &blocked_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "blocked",
                "--reason",
                "resume later",
            ],
        )
        .assert()
        .code(10);
    let blocked_stdout = stdout_text(&blocked);
    let blocked_anchor = parse_value("TASK_EXECUTION_ANCHOR", &blocked_stdout).unwrap();
    let blocked_bundle = path_from_stdout(&blocked_stdout, "BUNDLE_PATH");
    assert!(blocked_bundle.exists());
    assert!(blocked_bundle
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(&blocked_anchor));
    assert!(parse_value("RECEIPT_PATH", &blocked_stdout).is_none());
    assert!(bundle_listing(&blocked_bundle).contains(terminal_receipt_relative_path()));
    assert!(bundle_listing(&blocked_bundle).contains(repository_receipt_relative_path()));
    assert!(!blocked_worktree.exists());
    assert!(fixture.telegram_request_count() >= 1);

    let failed_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "failed"])
        .assert()
        .success();
    let failed_worktree = task_worktree_from_assert(&failed_open);
    write_file(&failed_worktree.join("failed.txt"), "failed\n");
    let failed = fixture
        .run_xtask(
            &failed_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "failed",
                "--reason",
                "simulated failure",
            ],
        )
        .assert()
        .code(11);
    let failed_stdout = stdout_text(&failed);
    let failed_anchor = parse_value("TASK_EXECUTION_ANCHOR", &failed_stdout).unwrap();
    let failed_bundle = path_from_stdout(&failed_stdout, "BUNDLE_PATH");
    assert!(failed_bundle.exists());
    assert!(failed_bundle
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(&failed_anchor));
    assert!(parse_value("RECEIPT_PATH", &failed_stdout).is_none());
    assert!(bundle_listing(&failed_bundle).contains(terminal_receipt_relative_path()));
    assert!(bundle_listing(&failed_bundle).contains(repository_receipt_relative_path()));
    assert!(!failed_worktree.exists());

    fixture.fail_telegram();
    let notify_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "notify-fail"])
        .assert()
        .success();
    let notify_worktree = task_worktree_from_assert(&notify_open);
    write_file(&notify_worktree.join("notify.txt"), "notify\n");
    fixture
        .run_xtask(
            &notify_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "failed",
                "--reason",
                "telegram unavailable",
            ],
        )
        .assert()
        .code(11);
    assert!(!notify_worktree.exists());

    let success_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "happy"])
        .assert()
        .success();
    let success_open_stdout = stdout_text(&success_open);
    let success_worktree = path_from_stdout(&success_open_stdout, "TASK_WORKTREE");
    let success_anchor = parse_value("TASK_EXECUTION_ANCHOR", &success_open_stdout).unwrap();
    let stale_open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "stale-merged"])
        .assert()
        .success();
    let stale_worktree = task_worktree_from_assert(&stale_open);
    write_file(&stale_worktree.join("stale.txt"), "stale\n");
    git(&stale_worktree, &["add", "stale.txt"]);
    git(&stale_worktree, &["commit", "-m", "stale residue"]);
    git(
        &fixture.primary,
        &["merge", "--ff-only", "task/stale-merged"],
    );
    git(&fixture.primary, &["push", "origin", BASE_BRANCH]);
    assert!(stale_worktree.exists());
    write_file(&success_worktree.join("happy.txt"), "happy\n");
    let remote_advanced_head = fixture.advance_base_branch();
    let success = fixture
        .run_xtask(
            &success_worktree,
            &[
                "task",
                "closeout",
                "--outcome",
                "success",
                "--message",
                "docs: close happy path",
            ],
        )
        .assert()
        .success();
    let success_stdout = stdout_text(&success);
    assert_eq!(
        parse_value("TASK_EXECUTION_ANCHOR", &success_stdout).unwrap(),
        success_anchor
    );
    assert!(!success_stdout.contains("TERMINAL_STATE="));
    assert!(!success_stdout.contains("TERMINAL_CHECK="));
    let success_bundle = path_from_stdout(&success_stdout, "BUNDLE_PATH");
    assert!(success_bundle.exists());
    assert!(success_bundle
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(&success_anchor));
    assert!(parse_value("RECEIPT_PATH", &success_stdout).is_none());
    let listing = bundle_listing(&success_bundle);
    assert!(listing.contains("metadata/bundle.env"));
    assert!(listing.contains("metadata/checksums.sha256"));
    assert!(listing.contains(terminal_receipt_relative_path()));
    assert!(listing.contains(repository_receipt_relative_path()));
    assert!(listing.contains("metadata/task-run-summary.json"));
    assert!(listing.contains("metadata/task-prompt.json"));
    assert!(listing.contains("metadata/task-prompt.txt"));
    assert!(listing.contains("evidence/submodules.txt"));
    assert!(listing.contains("logs/events.ndjson"));
    assert!(listing.contains("logs/task-open.log"));
    assert!(listing.contains("logs/summary/task-open.json"));
    assert!(listing.contains("repo/.githooks/pre-push"));
    assert!(!listing.contains("repo/scripts/ci/build-prebuilt-images.sh"));
    assert!(!listing.contains("metadata/task-run-events.ndjson"));
    assert!(!listing.contains("repo/.task/logs/"));
    assert!(!listing.contains("repo/.task/prompt/"));
    assert!(!listing.contains("repo/.task/task.env"));
    let canonical_event_count = listing
        .lines()
        .filter(|line| line.ends_with("logs/events.ndjson"))
        .count();
    assert_eq!(canonical_event_count, 1);
    let task_env_count = listing
        .lines()
        .filter(|line| line.ends_with("metadata/task.env"))
        .count();
    assert_eq!(task_env_count, 1);
    assert!(!success_worktree.exists());
    assert!(!stale_worktree.exists());
    let stale_branch = Command::new("git")
        .current_dir(&fixture.primary)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/task/stale-merged",
        ])
        .status()
        .unwrap();
    assert!(!stale_branch.success());
    let bundle_env = bundle_extract(&success_bundle, "metadata/bundle.env");
    assert!(bundle_env.contains(&format!("TASK_EXECUTION_ANCHOR='{}'", success_anchor)));
    assert!(bundle_env.contains("TASK_RUN_SUMMARY='metadata/task-run-summary.json'"));
    assert!(bundle_env.contains("TASK_RUN_EVENTS='logs/events.ndjson'"));
    assert!(bundle_env.contains("TASK_DESCRIPTION_FILE='metadata/task-description.txt'"));
    assert!(bundle_env.contains("TASK_DESCRIPTION='Execution task fixture prompt'"));
    assert!(bundle_env.contains("TASK_AGENT_RUNNER='codex'"));
    assert!(bundle_env.contains("TASK_AGENT_ID='fixture-agent'"));
    assert!(bundle_env.contains("TASK_AGENT_MODEL='gpt-5.4'"));
    assert!(bundle_env.contains("TASK_USAGE_TOTAL_TOKENS='321'"));
    assert!(bundle_env.contains("TASK_USAGE_INPUT_TOKENS='120'"));
    assert!(bundle_env.contains("TASK_USAGE_CACHED_INPUT_TOKENS='20'"));
    assert!(bundle_env.contains("TASK_USAGE_OUTPUT_TOKENS='181'"));
    assert!(bundle_env.contains("TASK_USAGE_CREDITS='1.25'"));
    assert!(bundle_env.contains("TASK_AGENT_STATUS='healthy'"));
    assert!(bundle_env.contains("TASK_AGENT_REMAINING_CAPACITY='42%'"));
    assert!(bundle_env.contains("TASK_PROMPT_ARCHIVED='true'"));
    assert!(bundle_env.contains("TASK_PROMPT_STATE='archived'"));
    assert!(bundle_env.contains("TASK_TELEMETRY_STATE='partial'"));
    assert!(bundle_env.contains("TASK_TELEMETRY_UNAVAILABLE_REASON='runner_exposed_subset_only'"));
    let task_head = bundle_env
        .lines()
        .find_map(|line| {
            line.strip_prefix("TASK_HEAD=")
                .map(|value| value.trim_matches('\'').to_string())
        })
        .unwrap();
    assert_eq!(
        git_output(&fixture.primary, &["rev-parse", BASE_BRANCH]),
        task_head
    );
    let remote_base_ref = format!("refs/remotes/origin/{BASE_BRANCH}");
    assert_eq!(
        git_output(&fixture.primary, &["rev-parse", &remote_base_ref]),
        task_head
    );
    let base_head_ref = format!("refs/heads/{BASE_BRANCH}");
    let remote_head = Command::new("git")
        .arg("--git-dir")
        .arg(&fixture.remote)
        .args(["rev-parse", &base_head_ref])
        .output()
        .unwrap();
    assert!(remote_head.status.success());
    assert_eq!(
        String::from_utf8(remote_head.stdout).unwrap().trim(),
        task_head
    );
    let merge_base = Command::new("git")
        .current_dir(&fixture.primary)
        .args([
            "merge-base",
            "--is-ancestor",
            &remote_advanced_head,
            &task_head,
        ])
        .status()
        .unwrap();
    assert!(merge_base.success());
    assert!(git_output(&fixture.primary, &["status", "--porcelain"]).is_empty());
    let validation_log = fs::read_to_string(&fixture.validation_log).unwrap();
    assert!(validation_log.contains("verify-autofix|"));
    assert!(validation_log.contains("verify-preflight|"));
    assert!(validation_log.contains("verify-fast|"));
    let runtime_root = format!(
        "/tmp/repository-taskflow/{}",
        success_worktree.file_name().unwrap().to_string_lossy()
    );
    assert!(validation_log.contains(&runtime_root));
    assert!(validation_log.contains(&format!("{runtime_root}/cargo-target/default")));
    let task_open_log = bundle_extract(&success_bundle, "logs/task-open.log");
    assert!(task_open_log.contains("status=success"));
    assert!(task_open_log.contains("raw_log=omitted"));
    assert!(!task_open_log.contains("cmd|"));
    let prompt_text = bundle_extract(&success_bundle, "metadata/task-prompt.txt");
    assert_eq!(prompt_text, "Execution task fixture prompt");
    let description_text = bundle_extract(&success_bundle, "metadata/task-description.txt");
    assert_eq!(description_text, "Execution task fixture prompt");
    let run_summary = bundle_extract(&success_bundle, "metadata/task-run-summary.json");
    assert!(run_summary.contains(&format!(
        "\"task_execution_anchor\": \"{}\"",
        success_anchor
    )));
    assert!(run_summary.contains("\"task_description\": \"Execution task fixture prompt\""));
    assert!(run_summary.contains("\"runner\": \"codex\""));
    assert!(run_summary.contains("\"total_tokens\": 321"));
    assert!(run_summary.contains("\"credits\": 1.25"));
    assert!(run_summary.contains("\"remaining_capacity\": \"42%\""));
    assert!(run_summary.contains("\"archived\": true"));
    assert!(run_summary.contains("\"state\": \"archived\""));
    assert!(run_summary.contains("\"telemetry\""));
    assert!(run_summary.contains("\"state\": \"partial\""));
    assert!(run_summary.contains("\"runner_exposed_subset_only\""));
    let receipt = bundle_extract_env(&success_bundle, terminal_receipt_relative_path());
    let repository_receipt =
        bundle_extract_env(&success_bundle, repository_receipt_relative_path());
    assert_eq!(receipt.get("OUTCOME").unwrap(), "success");
    assert_eq!(
        receipt.get("TASK_EXECUTION_ANCHOR").unwrap(),
        &success_anchor
    );
    assert_eq!(receipt.get("MESSAGE").unwrap(), "docs: close happy path");
    assert_eq!(receipt, repository_receipt);
    let telegram_request: Value =
        serde_json::from_str(&fixture.last_telegram_request().unwrap()).unwrap();
    assert_eq!(telegram_request["parse_mode"], "MarkdownV2");
    let telegram_text = telegram_request["text"].as_str().unwrap();
    assert!(telegram_text.contains("_Execution task fixture prompt_"));
    assert!(telegram_text.contains(&format!("*Anchor:* `{}`", success_anchor)));
    assert!(telegram_text.contains("*Message:* docs: close happy path"));
    assert!(
        telegram_text.contains("*Tokens:* `total=321 input=120 cached=20 output=181 credits=1.25`")
    );
    assert!(telegram_text.contains(
        "*Agent:* `runner=codex status=healthy remaining=42% model=gpt-5.4 id=fixture-agent`"
    ));
    assert!(!telegram_text.contains("telemetry: "));
    assert!(!telegram_text.contains("exit: "));
}
