use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum NodeDataStatus {
    Fresh,
    Stale,
    Partial,
    Unavailable,
}

impl NodeDataStatus {
    const fn priority(self) -> u8 {
        match self {
            Self::Fresh => 0,
            Self::Stale => 1,
            Self::Partial => 2,
            Self::Unavailable => 3,
        }
    }

    pub(crate) fn merge(self, other: Self) -> Self {
        if other.priority() > self.priority() {
            other
        } else {
            self
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeSnapshot {
    pub(crate) protocol_version: u32,
    pub(crate) status: NodeDataStatus,
    pub(crate) heartbeat: NodeHeartbeat,
    pub(crate) managed: NodeManagedSnapshot,
    pub(crate) queue: NodeQueueVisibility,
    pub(crate) resources: NodeResourceSnapshot,
    pub(crate) proxy: NodeProxySnapshot,
    pub(crate) issues: Vec<NodeSnapshotIssue>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeHeartbeat {
    pub(crate) status: NodeDataStatus,
    pub(crate) version: String,
    pub(crate) hostname: Option<String>,
    pub(crate) boot_id: Option<String>,
    pub(crate) sampled_at_unix: u64,
    pub(crate) monotonic_sample_ms: Option<u64>,
    pub(crate) pid: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeManagedSnapshot {
    pub(crate) status: NodeDataStatus,
    pub(crate) agents: Vec<NodeManagedAgent>,
    pub(crate) sessions: Vec<NodeManagedSession>,
    pub(crate) issues: Vec<NodeSnapshotIssue>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeManagedAgent {
    pub(crate) id: String,
    pub(crate) track: String,
    pub(crate) pid: u32,
    pub(crate) process_state: String,
    pub(crate) stale: bool,
    pub(crate) started_at_unix: u64,
    pub(crate) command: Vec<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) stdout_log_path: Option<String>,
    pub(crate) stderr_log_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeManagedSession {
    pub(crate) agent_id: String,
    pub(crate) track: String,
    pub(crate) session: String,
    pub(crate) pane: String,
    pub(crate) target: String,
    pub(crate) started_at_unix: u64,
    pub(crate) command: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeQueueVisibility {
    pub(crate) status: NodeDataStatus,
    pub(crate) count: usize,
    pub(crate) running: bool,
    pub(crate) active_tab_id: String,
    pub(crate) active_run: Option<NodeQueueRun>,
    pub(crate) active_items: Vec<NodeQueueItem>,
    pub(crate) tabs: Vec<NodeQueueTab>,
    pub(crate) issues: Vec<NodeSnapshotIssue>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeQueueRun {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) execution_mode: String,
    pub(crate) execution_host: String,
    pub(crate) selected_agent_command: String,
    pub(crate) selected_repo_root: Option<String>,
    pub(crate) selected_repo_name: Option<String>,
    pub(crate) remote_launcher: Option<String>,
    pub(crate) remote_agent_local_proxy: Option<String>,
    pub(crate) remote_agent_remote_proxy: Option<String>,
    pub(crate) current_index: i64,
    pub(crate) stop_requested: bool,
    pub(crate) message: String,
    pub(crate) created_at_unix: u64,
    pub(crate) updated_at_unix: u64,
    pub(crate) stale: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeQueueItem {
    pub(crate) id: String,
    pub(crate) run_id: String,
    pub(crate) position: i64,
    pub(crate) slug: String,
    pub(crate) status: String,
    pub(crate) execution_host: String,
    pub(crate) agent_command: String,
    pub(crate) agent_id: Option<String>,
    pub(crate) repo_root: Option<String>,
    pub(crate) repo_name: Option<String>,
    pub(crate) remote_launcher: Option<String>,
    pub(crate) remote_agent_local_proxy: Option<String>,
    pub(crate) remote_agent_remote_proxy: Option<String>,
    pub(crate) attempts: i64,
    pub(crate) recovery_attempts: i64,
    pub(crate) next_attempt_at_unix: Option<u64>,
    pub(crate) started_at_unix: u64,
    pub(crate) updated_at_unix: u64,
    pub(crate) stale: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct NodeQueueTab {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) run_id: Option<String>,
    pub(crate) status: String,
    pub(crate) running: bool,
    pub(crate) count: usize,
    pub(crate) active: bool,
    pub(crate) is_default: bool,
    pub(crate) updated_at_unix: u64,
    pub(crate) stale: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeResourceSnapshot {
    pub(crate) status: NodeDataStatus,
    pub(crate) cpu: Option<NodeCpuSnapshot>,
    pub(crate) load: Option<NodeLoadSnapshot>,
    pub(crate) memory: Option<NodeMemorySnapshot>,
    pub(crate) swap: Option<NodeSwapSnapshot>,
    pub(crate) disk: Option<NodeDiskSnapshot>,
    pub(crate) pids: Option<NodePidSnapshot>,
    pub(crate) io: Option<NodeIoSnapshot>,
    pub(crate) network: Option<NodeNetworkSnapshot>,
    pub(crate) issues: Vec<NodeSnapshotIssue>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeCpuSnapshot {
    pub(crate) logical_cpus: usize,
    pub(crate) total_jiffies: u64,
    pub(crate) idle_jiffies: u64,
    pub(crate) busy_jiffies: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeLoadSnapshot {
    pub(crate) one: f64,
    pub(crate) five: f64,
    pub(crate) fifteen: f64,
    pub(crate) running_processes: Option<u64>,
    pub(crate) total_processes: Option<u64>,
    pub(crate) last_pid: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[allow(clippy::struct_field_names)]
pub(crate) struct NodeMemorySnapshot {
    pub(crate) total_bytes: u64,
    pub(crate) available_bytes: u64,
    pub(crate) free_bytes: u64,
    pub(crate) used_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[allow(clippy::struct_field_names)]
pub(crate) struct NodeSwapSnapshot {
    pub(crate) total_bytes: u64,
    pub(crate) free_bytes: u64,
    pub(crate) used_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeDiskSnapshot {
    pub(crate) root: NodeDiskUsage,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeDiskUsage {
    pub(crate) mount_point: String,
    pub(crate) total_bytes: u64,
    pub(crate) available_bytes: u64,
    pub(crate) free_bytes: u64,
    pub(crate) used_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodePidSnapshot {
    pub(crate) count: usize,
    pub(crate) pid_max: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct NodeIoSnapshot {
    pub(crate) device_count: usize,
    pub(crate) read_ios: u64,
    pub(crate) write_ios: u64,
    pub(crate) sectors_read: u64,
    pub(crate) sectors_written: u64,
    pub(crate) io_in_progress: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeNetworkSnapshot {
    pub(crate) interface_count: usize,
    pub(crate) rx_bytes: u64,
    pub(crate) rx_packets: u64,
    pub(crate) rx_errors: u64,
    pub(crate) rx_dropped: u64,
    pub(crate) tx_bytes: u64,
    pub(crate) tx_packets: u64,
    pub(crate) tx_errors: u64,
    pub(crate) tx_dropped: u64,
    pub(crate) interfaces: Vec<NodeNetworkInterface>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeNetworkInterface {
    pub(crate) name: String,
    pub(crate) rx_bytes: u64,
    pub(crate) tx_bytes: u64,
    pub(crate) rx_packets: u64,
    pub(crate) tx_packets: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeProxySnapshot {
    pub(crate) status: NodeDataStatus,
    pub(crate) env: Vec<NodeProxyEnv>,
    pub(crate) port_forwards: Vec<NodePortForward>,
    pub(crate) issues: Vec<NodeSnapshotIssue>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeProxyEnv {
    pub(crate) name: String,
    pub(crate) value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodePortForward {
    pub(crate) source: String,
    pub(crate) direction: String,
    pub(crate) endpoint: String,
    pub(crate) observed_state: String,
    pub(crate) stale: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeSnapshotIssue {
    pub(crate) block: String,
    pub(crate) status: NodeDataStatus,
    pub(crate) message: String,
}
