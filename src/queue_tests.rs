#![allow(clippy::unwrap_used, reason = "queue parser tests use compact fixtures")]

use std::fs;

use super::*;

#[test]
fn json_plan_applies_default_layers() {
    let package = parse_json_package(
        r#"{
            "run_id": "batch-1",
            "execution_mode": "graph",
            "layers": [{"name":"policy","prompt":"shared policy"}],
            "items": [
                {"slug":"first","prompt":"do first"},
                {"slug":"second","prompt":"do second","depends_on":["first"]}
            ]
        }"#,
        "test",
    )
    .unwrap();

    assert_eq!(package.run_id.as_deref(), Some("batch-1"));
    assert_eq!(package.execution_mode.as_deref(), Some("graph"));
    assert_eq!(package.items.len(), 2);
    assert_eq!(package.items[0].slug.as_deref(), Some("first"));
    assert!(package.items[0].prompt.contains("[layer:policy]\nshared policy"));
    assert!(package.items[0].prompt.ends_with("do first"));
    assert_eq!(package.items[1].depends_on, vec!["first"]);
}

#[test]
fn json_plan_preserves_remote_launcher_hints() {
    let package = parse_json_package(
        r#"{
            "selected_remote_launcher": "remote-dev-env",
            "items": [
                {"slug":"remote","prompt":"do remote"},
                {"slug":"local","prompt":"do local","remote_launcher":"local"}
            ]
        }"#,
        "test",
    )
    .unwrap();

    assert_eq!(
        package.selected_remote_launcher.as_deref(),
        Some("remote-dev-env")
    );
    assert_eq!(
        package.items[1].remote_launcher.as_deref(),
        Some("local")
    );
}

#[test]
fn json_plan_preserves_remote_native_contract() {
    let package = parse_json_package(
        r#"{
            "selected_execution_host": "remote-native",
            "selected_remote_launcher": "remote-dev-env",
            "selected_remote_agent_local_proxy": "127.0.0.1:3128",
            "selected_remote_agent_remote_proxy": "127.0.0.1:18100",
            "items": [
                {"slug":"remote","prompt":"do remote"},
                {
                    "slug":"local",
                    "prompt":"do local",
                    "execution_host":"local",
                    "remote_agent_local_proxy":"none"
                }
            ]
        }"#,
        "test",
    )
    .unwrap();

    assert_eq!(
        package.selected_execution_host.as_deref(),
        Some("remote-native")
    );
    assert_eq!(
        package.selected_remote_agent_local_proxy.as_deref(),
        Some("127.0.0.1:3128")
    );
    assert_eq!(
        package.selected_remote_agent_remote_proxy.as_deref(),
        Some("127.0.0.1:18100")
    );
    assert_eq!(package.items[1].execution_host.as_deref(), Some("local"));
    assert_eq!(
        package.items[1].remote_agent_local_proxy.as_deref(),
        Some("none")
    );
}

#[test]
fn json_item_layers_can_override_defaults() {
    let package = parse_json_package(
        r#"{
            "layers": {
                "base": "base layer",
                "extra": "extra layer"
            },
            "default_layers": ["base"],
            "items": [
                {"prompt":"one"},
                {"prompt":"two","layers":["extra"]}
            ]
        }"#,
        "test",
    )
    .unwrap();

    assert!(package.items[0].prompt.contains("[layer:base]"));
    assert!(!package.items[0].prompt.contains("[layer:extra]"));
    assert!(package.items[1].prompt.contains("[layer:extra]"));
    assert!(!package.items[1].prompt.contains("[layer:base]"));
}

#[test]
fn directory_package_uses_layers_and_prompt_dirs() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("layers")).unwrap();
    fs::create_dir(temp.path().join("prompts")).unwrap();
    fs::write(temp.path().join("layers/base.md"), "shared").unwrap();
    fs::write(temp.path().join("prompts/001-first.md"), "first").unwrap();
    fs::write(temp.path().join("ignored.txt"), "fallback").unwrap();

    let package = load_directory_package(temp.path()).unwrap();

    assert_eq!(package.items.len(), 1);
    assert_eq!(package.items[0].slug.as_deref(), Some("001-first"));
    assert!(package.items[0].prompt.contains("[layer:base]\nshared"));
    assert!(package.items[0].prompt.ends_with("first"));
}

#[test]
fn help_mentions_console_queue_flow() {
    assert!(help_text().contains("qcold queue run --from queue.json"));
    assert!(help_text().contains("qcold queue create"));
    assert!(help_text().contains("qcold queue switch"));
    assert!(help_text().contains("layers/*.md"));
}

#[test]
fn queue_list_hides_inactive_empty_tabs() {
    let tab = queue_tab_fixture("empty", false, None);

    assert!(!queue_tab_visible_in_list(&tab, None));
}

#[test]
fn queue_list_keeps_active_empty_tab() {
    let tab = queue_tab_fixture("empty", true, None);

    assert!(queue_tab_visible_in_list(&tab, None));
}

#[test]
fn queue_list_keeps_run_backed_tabs() {
    let tab = queue_tab_fixture("run-tab", false, Some("run-1"));
    let run = queue_run_fixture("run-1");

    assert!(queue_tab_visible_in_list(&tab, Some(&run)));
}

fn queue_tab_fixture(id: &str, active: bool, run_id: Option<&str>) -> state::QueueTabRow {
    state::QueueTabRow {
        id: id.to_string(),
        label: id.to_string(),
        run_id: run_id.map(ToString::to_string),
        is_default: false,
        active,
        created_at: 1,
        updated_at: 1,
    }
}

fn queue_run_fixture(id: &str) -> (state::QueueRunRow, Vec<state::QueueItemRow>) {
    (
        state::QueueRunRow {
            id: id.to_string(),
            status: "failed".into(),
            execution_mode: "graph".into(),
            execution_host: "local".into(),
            selected_agent_command: "c1".to_string(),
            remote_launcher: None,
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            selected_repo_root: None,
            selected_repo_name: None,
            track: "queue".to_string(),
            current_index: -1,
            stop_requested: false,
            message: String::new(),
            created_at: 1,
            updated_at: 1,
        },
        Vec::new(),
    )
}
