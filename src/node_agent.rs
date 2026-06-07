mod collect;
mod protocol;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

pub(crate) use protocol::NodeSnapshot;

#[derive(Args)]
pub(crate) struct NodeArgs {
    #[command(subcommand)]
    command: NodeCommand,
}

#[derive(Subcommand)]
enum NodeCommand {
    #[command(about = "Print a typed remote-node monitoring snapshot as JSON")]
    Snapshot(NodeSnapshotArgs),
}

#[derive(Args)]
struct NodeSnapshotArgs {
    #[arg(long, help = "Fetch a snapshot from a dashboard base URL or /api/node/snapshot URL")]
    endpoint: Option<String>,
    #[arg(long, help = "Pretty-print JSON output")]
    pretty: bool,
}

pub(crate) fn run(args: NodeArgs) -> Result<u8> {
    match args.command {
        NodeCommand::Snapshot(args) => {
            let snapshot = if let Some(endpoint) = args.endpoint.as_deref() {
                fetch_remote_snapshot(endpoint)?
            } else {
                collect_snapshot()
            };
            let json = if args.pretty {
                serde_json::to_string_pretty(&snapshot)?
            } else {
                serde_json::to_string(&snapshot)?
            };
            println!("{json}");
        }
    }
    Ok(0)
}

pub(crate) fn collect_snapshot() -> NodeSnapshot {
    collect::collect_snapshot()
}

fn fetch_remote_snapshot(endpoint: &str) -> Result<NodeSnapshot> {
    let url = node_snapshot_url(endpoint);
    let response = match ureq::get(&url).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            bail!("node snapshot request failed with HTTP {code}: {body}");
        }
        Err(err) => bail!("node snapshot request failed: {err}"),
    };
    let body = response
        .into_string()
        .with_context(|| format!("failed to read node snapshot response from {url}"))?;
    serde_json::from_str(&body).with_context(|| format!("failed to decode node snapshot from {url}"))
}

fn node_snapshot_url(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.ends_with("/api/node/snapshot") {
        endpoint.to_string()
    } else {
        format!("{endpoint}/api/node/snapshot")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn node_snapshot_url_accepts_base_or_api_path() {
        assert_eq!(
            node_snapshot_url("http://127.0.0.1:8787"),
            "http://127.0.0.1:8787/api/node/snapshot"
        );
        assert_eq!(
            node_snapshot_url("http://127.0.0.1:8787/api/node/snapshot"),
            "http://127.0.0.1:8787/api/node/snapshot"
        );
    }
}
