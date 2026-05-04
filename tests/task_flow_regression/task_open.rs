use super::*;

pub(crate) fn task_open_accepts_clean_materialized_submodule_tree_and_can_resume_remote_tasks() {
    let fixture = Fixture::new();

    git(
        &fixture.primary,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
            "cpp-btree",
        ],
    );
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "drift"])
        .assert()
        .success()
        .stdout(contains("[task-open] worktree action=create"))
        .stdout(contains("[task-open] devcontainer action=ready"));
    let worktree = task_worktree_from_assert(&open);
    assert!(submodule_materialized(&fixture.primary, "cpp-btree"));
    assert!(!submodule_materialized(&fixture.primary, "json11"));
    assert!(submodule_materialized(&worktree, "cpp-btree"));
    assert!(submodule_materialized(&worktree, "json11"));

    let primary_clean = fixture.temp.path().join("primary-clean");
    git_clone(&fixture.remote, &primary_clean);
    git(&primary_clean, &["config", "user.name", "tester"]);
    git(
        &primary_clean,
        &["config", "user.email", "tester@example.com"],
    );
    git(
        &primary_clean,
        &["config", "taskflow.base-branch", BASE_BRANCH],
    );
    let _ = Command::new("git")
        .current_dir(&primary_clean)
        .args(["remote", "set-head", "origin", BASE_BRANCH])
        .status();
    fixture.advance_base_branch();
    let stale_head = git_output(&primary_clean, &["rev-parse", "HEAD"]);
    let open_after_sync = fixture
        .run_xtask(&primary_clean, &["task", "open", "stale"])
        .assert()
        .success()
        .stdout(contains("[task-open] primary action=sync"))
        .stdout(contains("result=fast-forwarded"));
    let synced_worktree = task_worktree_from_assert(&open_after_sync);
    assert_ne!(
        git_output(&primary_clean, &["rev-parse", "HEAD"]),
        stale_head
    );
    assert!(synced_worktree.exists());

    let open = fixture
        .run_xtask(&primary_clean, &["task", "open", "reopen"])
        .assert()
        .success()
        .stdout(contains("[task-open] worktree action=create"))
        .stdout(contains("[task-open] devcontainer action=ready"));
    let worktree = task_worktree_from_assert(&open);
    assert!(submodule_materialized(&worktree, "cpp-btree"));
    assert!(submodule_materialized(&worktree, "json11"));
    assert!(!submodule_materialized(&primary_clean, "cpp-btree"));
    assert!(!submodule_materialized(&primary_clean, "json11"));

    write_file(&worktree.join("task.txt"), "task-commit\n");
    git(&worktree, &["add", "task.txt"]);
    git(&worktree, &["commit", "-m", "reopen"]);
    git(&worktree, &["push", "-u", "origin", "task/reopen"]);
    let head = git_output(&worktree, &["rev-parse", "HEAD"]);
    git(
        &primary_clean,
        &["worktree", "remove", "--force", worktree.to_str().unwrap()],
    );
    git(&primary_clean, &["branch", "-D", "task/reopen"]);

    let resume = fixture
        .run_xtask(&primary_clean, &["task", "open", "reopen"])
        .assert()
        .success();
    let resumed = task_worktree_from_assert(&resume);
    assert_eq!(git_output(&resumed, &["rev-parse", "HEAD"]), head);
}

pub(crate) fn task_open_refuses_dirty_primary_without_scrubbing() {
    let fixture = Fixture::new();

    write_file(&fixture.primary.join("dirty.txt"), "dirty\n");

    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "dirty-primary"])
        .assert()
        .failure()
        .stderr(contains("primary checkout is dirty:"))
        .stderr(contains("dirty_paths=dirty.txt"));
    assert!(open.get_output().stdout.is_empty());
    assert!(fixture.primary.join("dirty.txt").exists());
    assert!(!submodule_materialized(&fixture.primary, "cpp-btree"));
    assert!(!submodule_materialized(&fixture.primary, "json11"));
}

pub(crate) fn task_open_full_qemu_profile_uses_full_qemu_devcontainer_config() {
    let fixture = Fixture::new();

    let open = fixture
        .run_xtask(
            &fixture.primary,
            &["task", "open", "full-rust-e2e", "full-qemu"],
        )
        .assert()
        .success();
    let worktree = task_worktree_from_assert(&open);
    let devcontainer_log =
        fs::read_to_string(fixture.temp.path().join("devcontainer.log")).unwrap();
    let task_env = load_task_env(&worktree);
    let expected_config = worktree.join(".task/devcontainer/full-qemu/devcontainer.json");
    assert!(
        devcontainer_log.contains(&format!(
            "up|{}|{}|task/full-rust-e2e|{}",
            worktree.display(),
            expected_config.display(),
            task_env.devcontainer_id
        )),
        "unexpected devcontainer log:
{devcontainer_log}"
    );

    assert_eq!(task_env.task_profile, "full-qemu");
}

pub(crate) fn task_open_resume_reenters_managed_devcontainer_shell_and_marks_host_worktree_orchestration_only(
) {
    let fixture = Fixture::new();

    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "resume-shell"])
        .assert()
        .success()
        .stdout(contains("[task-open] host-worktree=orchestration-only"));
    let worktree = task_worktree_from_assert(&open);
    let first_log = fixture.devcontainer_log_text();
    let expected_exec = format!(
        "docker-exec|cid-{}|{}|bash",
        worktree.file_name().unwrap().to_string_lossy(),
        worktree.display()
    );
    assert!(
        first_log.contains(&expected_exec),
        "task-open did not enter the managed devcontainer shell on initial open:\n{first_log}"
    );
    assert!(worktree
        .join(".task/devcontainer/fast/devcontainer.json")
        .exists());

    fixture.clear_devcontainer_log();
    let reopen = fixture
        .run_xtask(&fixture.primary, &["task", "open", "resume-shell"])
        .assert()
        .success()
        .stdout(contains("[task-open] worktree action=resume"))
        .stdout(contains("[task-open] host-worktree=orchestration-only"));
    let resumed = task_worktree_from_assert(&reopen);
    assert_eq!(resumed, worktree);
    let reopen_log = fixture.devcontainer_log_text();
    assert!(
        reopen_log.contains(&expected_exec),
        "task-open did not re-enter the managed devcontainer shell on resume:\n{reopen_log}"
    );
}

pub(crate) fn task_open_mounts_notification_env_file_into_generated_devcontainer_config() {
    let fixture = Fixture::new();

    write_file(
        &fixture.primary.join(".git/info/exclude"),
        ".env.taskflow-telegram.local\n",
    );
    write_file(
        &fixture.primary.join(".env.taskflow-telegram.local"),
        "TELEGRAM_BOT_TOKEN=test-token\nTELEGRAM_CHAT_ID=test-chat\n",
    );

    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "notify-env"])
        .assert()
        .success();
    let worktree = task_worktree_from_assert(&open);
    let generated =
        fs::read_to_string(worktree.join(".task/devcontainer/fast/devcontainer.json")).unwrap();
    let mounted_target = worktree.join(".task/.env.taskflow-telegram.local");
    let mounted_source = fixture.primary.join(".env.taskflow-telegram.local");

    assert!(generated.contains(&format!(
        "\"source={},target={},type=bind,readonly\"",
        mounted_source.display(),
        mounted_target.display()
    )));
    assert!(generated.contains(&format!(
        "\"TELEGRAM_ENV_FILE\": \"{}\"",
        mounted_target.display()
    )));
    assert!(generated.contains(&format!(
        "\"JIRA_ENV_FILE\": \"{}\"",
        mounted_target.display()
    )));
}

pub(crate) fn task_open_full_qemu_profile_uses_prebuilt_image_override_when_configured() {
    let fixture = Fixture::new();

    let open = fixture
        .run_xtask(
            &fixture.primary,
            &["task", "open", "full-rust-e2e", "full-qemu"],
        )
        .env(
            "FULL_QEMU_IMAGE_REF",
            "registry.example.com/repository/ci:test-full-qemu",
        )
        .assert()
        .success()
        .stdout(contains(
            "[task-open] devcontainer action=prebuilt profile=full-qemu image_ref=registry.example.com/repository/ci:test-full-qemu",
        ));
    let worktree = task_worktree_from_assert(&open);
    let devcontainer_log =
        fs::read_to_string(fixture.temp.path().join("devcontainer.log")).unwrap();
    let task_env = load_task_env(&worktree);
    let expected_config = worktree.join(".task/devcontainer/full-qemu-prebuilt/devcontainer.json");
    assert!(
        devcontainer_log.contains(&format!(
            "up|{}|{}|task/full-rust-e2e|{}",
            worktree.display(),
            expected_config.display(),
            task_env.devcontainer_id
        )),
        "unexpected devcontainer log:
{devcontainer_log}"
    );

    let generated = fs::read_to_string(&expected_config).unwrap();
    assert!(generated.contains("\"image\": \"registry.example.com/repository/ci:test-full-qemu\""));
    assert!(!generated.contains("\"build\""));
    assert!(generated.contains("\"QCOLD_DEVCONTAINER_PROFILE\": \"full-qemu\""));
}
