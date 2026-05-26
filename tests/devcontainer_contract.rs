#![allow(
    missing_docs,
    reason = "integration tests document repository-local devcontainer contracts"
)]

use std::fs;
use std::path::Path;

use serde_json::Value;

#[test]
fn default_devcontainer_is_e2e_and_slim_is_explicit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let default = read_json(&root.join(".devcontainer/devcontainer.json"));
    let slim = read_json(&root.join(".devcontainer/slim/devcontainer.json"));

    assert_eq!(default["build"]["target"], "devcontainer-e2e");
    assert_eq!(default["containerEnv"]["QCOLD_DEVCONTAINER_PROFILE"], "e2e");
    assert!(default["features"]
        .as_object()
        .unwrap()
        .contains_key("ghcr.io/devcontainers/features/docker-outside-of-docker:1"));

    assert_eq!(slim["build"]["target"], "devcontainer-slim");
    assert_eq!(slim["containerEnv"]["QCOLD_DEVCONTAINER_PROFILE"], "slim");
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}
