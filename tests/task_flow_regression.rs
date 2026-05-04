#![allow(
    missing_docs,
    clippy::cast_possible_truncation,
    clippy::cognitive_complexity,
    clippy::expect_used,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::used_underscore_binding,
    clippy::unwrap_used,
    reason = "qcold integration tests validate orchestration task-flow behavior rather than a documented public API"
)]

//! Regression coverage for the qcold-owned managed task-flow contract.

#[path = "task_flow_regression/fixture.rs"]
mod fixture;
#[path = "task_flow_regression/helpers.rs"]
mod helpers;
#[path = "support/task_flow_helpers.rs"]
mod task_flow_helpers;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use predicates::str::contains;
use serde_json::Value;

use fixture::{git_clone, submodule_materialized, Fixture, BASE_BRANCH};
use helpers::{path_from_stdout, stdout_text, task_worktree_from_assert};
use task_flow_helpers::{
    bundle_extract, bundle_extract_env, bundle_listing, git, git_output, load_task_env,
    parse_value, repository_receipt_relative_path, save_task_env, terminal_receipt_relative_path,
    write_exe, write_file, TaskStatus,
};

#[cfg(unix)]
use std::os::unix::fs::symlink;

fn bundle_listing_has_path(listing: &str, path: &str) -> bool {
    listing
        .lines()
        .filter_map(|line| line.strip_prefix("Path = "))
        .any(|entry| entry == path)
}

fn bundle_file_paths(listing: &str) -> BTreeSet<String> {
    let mut files = Vec::new();
    let mut current_path = None::<String>;
    let mut current_is_folder = false;
    for line in listing.lines() {
        if line.is_empty() {
            if let Some(path) = current_path.take() {
                if !current_is_folder {
                    files.push(path);
                }
            }
            current_is_folder = false;
            continue;
        }
        if let Some(path) = line.strip_prefix("Path = ") {
            current_path = Some(path.to_string());
        } else if let Some(folder) = line.strip_prefix("Folder = ") {
            current_is_folder = folder == "+";
        }
    }
    if let Some(path) = current_path.take() {
        if !current_is_folder {
            files.push(path);
        }
    }
    files.into_iter().skip(1).collect()
}

fn file_manifest_paths(manifest: &str) -> BTreeSet<String> {
    manifest
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            match (fields.next(), fields.next()) {
                (Some("file" | "symlink"), Some(entry)) => Some(entry.to_string()),
                _ => None,
            }
        })
        .collect()
}

fn file_manifest_has_path(manifest: &str, path: &str) -> bool {
    manifest.lines().any(|line| {
        let mut fields = line.split('\t');
        matches!(
            (fields.next(), fields.next()),
            (Some("file" | "symlink"), Some(entry)) if entry == path
        )
    })
}

fn checksum_manifest_paths(manifest: &str) -> BTreeSet<String> {
    manifest
        .lines()
        .filter_map(|line| line.split_once("  ").map(|(_, path)| path.to_string()))
        .collect()
}

fn checksum_manifest_has_path(manifest: &str, path: &str) -> bool {
    manifest
        .lines()
        .any(|line| line.ends_with(&format!("  {path}")))
}

#[cfg(unix)]
fn seed_snapshot_hardening_artifacts(root: &Path) {
    let gitignore_path = root.join(".gitignore");
    let mut gitignore = fs::read_to_string(&gitignore_path).unwrap_or_default();
    gitignore.push_str(".env.taskflow-telegram.local\nfio\nqemu\n");
    fs::write(&gitignore_path, gitignore).unwrap();
    fs::write(root.join("snapshot-source.txt"), "payload\n").unwrap();
    symlink("snapshot-source.txt", root.join("snapshot-link")).unwrap();
    fs::write(
        root.join(".env.taskflow-telegram.local"),
        "TELEGRAM_BOT_TOKEN=redacted-token\nTELEGRAM_CHAT_ID=redacted-chat\n",
    )
    .unwrap();
    symlink("/opt/src/fio", root.join("fio")).unwrap();
    symlink("/opt/src/qemu", root.join("qemu")).unwrap();
}

fn assert_snapshot_hardening(bundle: &Path) {
    let listing = bundle_listing(bundle);
    let archive_files = bundle_file_paths(&listing);
    let file_manifest = bundle_extract(bundle, "metadata/file-manifest.txt");
    let checksum_manifest = bundle_extract(bundle, "metadata/checksums.sha256");
    let bundle_manifest = bundle_extract(bundle, "metadata/bundle-manifest.txt");

    assert!(bundle_manifest.contains(
        "ARCHIVE_NORMALIZATION=sorted-paths,utc-fixed-mtime,zip-deflate,symlink-preserve"
    ));
    assert!(bundle_manifest
        .contains("REPO_SNAPSHOT_SELECTION=tracked paths from git ls-files --cached -z"));
    assert!(bundle_manifest.contains(
        "REPO_SNAPSHOT_SELECTION=untracked non-ignored paths from git ls-files --others --exclude-standard -z"
    ));
    assert!(
        !archive_files.iter().any(|path| path.starts_with(".tmp")),
        "unexpected temp-root archive entry present: {archive_files:?}"
    );

    let file_manifest_paths = file_manifest_paths(&file_manifest);
    let checksum_manifest_paths = checksum_manifest_paths(&checksum_manifest);
    assert_eq!(file_manifest_paths, checksum_manifest_paths);

    let mut expected_archive_files = file_manifest_paths;
    expected_archive_files.insert("metadata/file-manifest.txt".to_string());
    expected_archive_files.insert("metadata/checksums.sha256".to_string());
    assert_eq!(archive_files, expected_archive_files);

    let path = "repo/snapshot-link";
    assert!(
        bundle_listing_has_path(&listing, path),
        "missing bundle entry {path}"
    );
    assert!(
        file_manifest_has_path(&file_manifest, path),
        "missing file manifest entry {path}"
    );
    assert!(
        checksum_manifest_has_path(&checksum_manifest, path),
        "missing checksum manifest entry {path}"
    );

    for path in ["repo/.env.taskflow-telegram.local", "repo/fio", "repo/qemu"] {
        assert!(
            !bundle_listing_has_path(&listing, path),
            "unexpected bundle entry {path}"
        );
        assert!(
            !file_manifest_has_path(&file_manifest, path),
            "unexpected file manifest entry {path}"
        );
        assert!(
            !checksum_manifest_has_path(&checksum_manifest, path),
            "unexpected checksum manifest entry {path}"
        );
    }
}

fn write_incompressible_blob(path: &Path, len: usize) {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut data = vec![0u8; len];
    for byte in &mut data {
        state ^= state << 7;
        state ^= state >> 9;
        state ^= state << 8;
        *byte = state as u8;
    }
    fs::write(path, data).unwrap();
}

#[path = "task_flow_regression/bundles.rs"]
mod bundles;
#[path = "task_flow_regression/closeout_contracts.rs"]
mod closeout_contracts;
#[path = "task_flow_regression/closeout_failures.rs"]
mod closeout_failures;
#[path = "task_flow_regression/delivery.rs"]
mod delivery;
#[path = "task_flow_regression/notifications_runtime.rs"]
mod notifications_runtime;
#[path = "task_flow_regression/task_open.rs"]
mod task_open;

#[test]
fn task_open_accepts_clean_materialized_submodule_tree_and_can_resume_remote_tasks() {
    task_open::task_open_accepts_clean_materialized_submodule_tree_and_can_resume_remote_tasks();
}

#[test]
fn task_open_refuses_dirty_primary_without_scrubbing() {
    task_open::task_open_refuses_dirty_primary_without_scrubbing();
}

#[test]
fn task_open_full_qemu_profile_uses_full_qemu_devcontainer_config() {
    task_open::task_open_full_qemu_profile_uses_full_qemu_devcontainer_config();
}

#[test]
fn task_open_resume_reenters_managed_devcontainer_shell_and_marks_host_worktree_orchestration_only()
{
    task_open::task_open_resume_reenters_managed_devcontainer_shell_and_marks_host_worktree_orchestration_only();
}

#[test]
fn task_open_mounts_notification_env_file_into_generated_devcontainer_config() {
    task_open::task_open_mounts_notification_env_file_into_generated_devcontainer_config();
}

#[test]
fn task_open_full_qemu_profile_uses_prebuilt_image_override_when_configured() {
    task_open::task_open_full_qemu_profile_uses_prebuilt_image_override_when_configured();
}

#[test]
fn current_bundle_snapshot_hardening_excludes_local_telegram_env_and_keeps_symlinks_consistent() {
    bundles::current_bundle_snapshot_hardening_excludes_local_telegram_env_and_keeps_symlinks_consistent();
}

#[test]
fn current_bundle_rejects_oversized_archive_payloads() {
    bundles::current_bundle_rejects_oversized_archive_payloads();
}

#[test]
fn task_bundle_snapshot_hardening_excludes_local_telegram_env_and_keeps_symlinks_consistent() {
    bundles::task_bundle_snapshot_hardening_excludes_local_telegram_env_and_keeps_symlinks_consistent();
}

#[test]
fn blocked_failed_and_success_closeout_paths_preserve_terminal_contracts() {
    closeout_contracts::blocked_failed_and_success_closeout_paths_preserve_terminal_contracts();
}

#[test]
fn markdown_only_success_closeout_skips_canonical_validation() {
    closeout_failures::markdown_only_success_closeout_skips_canonical_validation();
}

#[test]
fn success_closeout_omits_repository_terminal_state_when_other_tasks_remain_open() {
    closeout_failures::success_closeout_omits_repository_terminal_state_when_other_tasks_remain_open();
}

#[test]
fn incomplete_success_closeout_emits_failed_bundle_and_preserves_worktree() {
    closeout_failures::incomplete_success_closeout_emits_failed_bundle_and_preserves_worktree();
}

#[test]
fn post_delivery_cleanup_failure_is_logged_without_blocking_success_closeout() {
    closeout_failures::post_delivery_cleanup_failure_is_logged_without_blocking_success_closeout();
}

#[test]
fn success_closeout_fails_before_validation_when_primary_dirty_overlaps_open_task() {
    closeout_failures::success_closeout_fails_before_validation_when_primary_dirty_overlaps_open_task();
}

#[test]
fn success_closeout_allows_failed_closeout_task_residue_and_leaves_terminal_check_non_terminal() {
    closeout_failures::success_closeout_allows_failed_closeout_task_residue_and_leaves_terminal_check_non_terminal();
}

#[test]
fn cleanup_failure_scrubs_terminal_receipt_from_incomplete_bundle() {
    closeout_failures::cleanup_failure_scrubs_terminal_receipt_from_incomplete_bundle();
}

#[test]
fn late_closeout_failure_preserves_precleanup_bundle_and_keeps_git_worktree_valid() {
    closeout_failures::late_closeout_failure_preserves_precleanup_bundle_and_keeps_git_worktree_valid();
}

#[test]
fn success_closeout_delivers_directly_and_records_delivery_metadata() {
    delivery::success_closeout_delivers_directly_and_records_delivery_metadata();
}

#[test]
fn success_closeout_treats_legacy_merge_request_mode_as_direct() {
    delivery::success_closeout_treats_legacy_merge_request_mode_as_direct();
}

#[test]
fn iteration_notify_sends_non_terminal_handoff_message_and_preserves_task_state() {
    notifications_runtime::iteration_notify_sends_non_terminal_handoff_message_and_preserves_task_state();
}

#[test]
fn verify_preflight_runs_directly_inside_container_runtime() {
    notifications_runtime::verify_preflight_runs_directly_inside_container_runtime();
}
