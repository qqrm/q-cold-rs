use super::*;

#[cfg(unix)]
pub(crate) fn current_bundle_snapshot_hardening_excludes_local_telegram_env_and_keeps_symlinks_consistent(
) {
    let fixture = Fixture::new();
    seed_snapshot_hardening_artifacts(&fixture.primary);

    let current = fixture
        .run_xtask(&fixture.primary, &["task", "bundle"])
        .assert()
        .success();
    let current_bundle = path_from_stdout(&stdout_text(&current), "BUNDLE_PATH");
    assert_snapshot_hardening(&current_bundle);
}

pub(crate) fn current_bundle_rejects_oversized_archive_payloads() {
    let fixture = Fixture::new();
    write_incompressible_blob(&fixture.primary.join("oversized.bin"), 6 * 1024 * 1024);

    let bundle = fixture
        .run_xtask(&fixture.primary, &["task", "bundle"])
        .assert()
        .code(1)
        .stderr(contains("bundle size"))
        .stderr(contains("exceeds limit 5242880 bytes"))
        .stderr(contains("oversized bundle removed"))
        .stderr(contains("largest staged entries:"))
        .stderr(contains("repo/oversized.bin"));
    let stdout = stdout_text(&bundle);
    assert!(parse_value("BUNDLE_PATH", &stdout).is_none());

    let bundles_dir = fixture.primary.join("bundles");
    if bundles_dir.exists() {
        assert_eq!(fs::read_dir(bundles_dir).unwrap().count(), 0);
    }
}

#[cfg(unix)]
pub(crate) fn task_bundle_snapshot_hardening_excludes_local_telegram_env_and_keeps_symlinks_consistent(
) {
    let fixture = Fixture::new();
    let open = fixture
        .run_xtask(&fixture.primary, &["task", "open", "snapshot-hardening"])
        .assert()
        .success();
    let worktree = path_from_stdout(&stdout_text(&open), "TASK_WORKTREE");
    seed_snapshot_hardening_artifacts(&worktree);

    let task_bundle = fixture
        .run_xtask(&fixture.primary, &["task", "bundle", "snapshot-hardening"])
        .assert()
        .success();
    let task_bundle_path = path_from_stdout(&stdout_text(&task_bundle), "BUNDLE_PATH");
    assert_snapshot_hardening(&task_bundle_path);
}
