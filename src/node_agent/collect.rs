use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::{agents, state};

use super::protocol::{
    NodeCpuSnapshot, NodeDataStatus, NodeDiskSnapshot, NodeDiskUsage, NodeHeartbeat,
    NodeIoSnapshot, NodeLoadSnapshot, NodeManagedAgent, NodeManagedSession, NodeManagedSnapshot,
    NodeMemorySnapshot, NodeNetworkInterface, NodeNetworkSnapshot, NodePidSnapshot,
    NodePortForward, NodeProxyEnv, NodeProxySnapshot, NodeQueueItem, NodeQueueRun, NodeQueueTab,
    NodeQueueVisibility, NodeResourceSnapshot, NodeSnapshot, NodeSnapshotIssue, NodeSwapSnapshot,
};

const NODE_PROTOCOL_VERSION: u32 = 1;
const QUEUE_STALE_SECONDS: u64 = 5 * 60;

pub(super) fn collect_snapshot() -> NodeSnapshot {
    let sampled_at_unix = unix_now();
    let heartbeat = collect_heartbeat(sampled_at_unix);
    let managed = collect_managed_snapshot();
    let queue = collect_queue_visibility(sampled_at_unix);
    let resources = collect_resource_snapshot();
    let proxy = collect_proxy_snapshot(&queue);
    let mut status = heartbeat.status;
    for next in [managed.status, queue.status, resources.status, proxy.status] {
        status = status.merge(next);
    }
    let issues = heartbeat_issues(&heartbeat)
        .into_iter()
        .chain(managed.issues.clone())
        .chain(queue.issues.clone())
        .chain(resources.issues.clone())
        .chain(proxy.issues.clone())
        .collect();
    NodeSnapshot {
        protocol_version: NODE_PROTOCOL_VERSION,
        status,
        heartbeat,
        managed,
        queue,
        resources,
        proxy,
        issues,
    }
}

fn collect_heartbeat(sampled_at_unix: u64) -> NodeHeartbeat {
    let hostname = read_trimmed("/proc/sys/kernel/hostname").or_else(|| read_trimmed("/etc/hostname"));
    let boot_id = read_trimmed("/proc/sys/kernel/random/boot_id");
    let monotonic_sample_ms = read_trimmed("/proc/uptime").and_then(|raw| parse_uptime_ms(&raw));
    let status = if hostname.is_some() && boot_id.is_some() && monotonic_sample_ms.is_some() {
        NodeDataStatus::Fresh
    } else {
        NodeDataStatus::Partial
    };
    NodeHeartbeat {
        status,
        version: crate::QCOLD_VERSION.to_string(),
        hostname,
        boot_id,
        sampled_at_unix,
        monotonic_sample_ms,
        pid: std::process::id(),
    }
}

fn heartbeat_issues(heartbeat: &NodeHeartbeat) -> Vec<NodeSnapshotIssue> {
    let mut issues = Vec::new();
    if heartbeat.hostname.is_none() {
        issues.push(issue("heartbeat", NodeDataStatus::Partial, "hostname unavailable"));
    }
    if heartbeat.boot_id.is_none() {
        issues.push(issue("heartbeat", NodeDataStatus::Partial, "boot id unavailable"));
    }
    if heartbeat.monotonic_sample_ms.is_none() {
        issues.push(issue(
            "heartbeat",
            NodeDataStatus::Partial,
            "monotonic sample timestamp unavailable",
        ));
    }
    issues
}

fn collect_managed_snapshot() -> NodeManagedSnapshot {
    let mut issues = Vec::new();
    let agents = match state::load_agents(&legacy_agents_path()) {
        Ok(rows) => rows.into_iter().map(node_managed_agent).collect(),
        Err(err) => {
            issues.push(issue(
                "managed",
                NodeDataStatus::Partial,
                format!("managed agent rows unavailable: {err:#}"),
            ));
            Vec::new()
        }
    };
    let sessions = match agents::terminal_contexts() {
        Ok(rows) => rows.into_iter().map(node_managed_session).collect(),
        Err(err) => {
            issues.push(issue(
                "managed",
                NodeDataStatus::Partial,
                format!("managed terminal sessions unavailable: {err:#}"),
            ));
            Vec::new()
        }
    };
    let stale_agents = agents.iter().filter(|agent: &&NodeManagedAgent| agent.stale).count();
    if stale_agents > 0 {
        issues.push(issue(
            "managed",
            NodeDataStatus::Stale,
            format!("stale managed agent rows present: {stale_agents}"),
        ));
    }
    let status = if issues.iter().any(|issue| issue.status == NodeDataStatus::Partial) {
        NodeDataStatus::Partial
    } else if stale_agents > 0 {
        NodeDataStatus::Stale
    } else {
        NodeDataStatus::Fresh
    };
    NodeManagedSnapshot {
        status,
        agents,
        sessions,
        issues,
    }
}

fn node_managed_agent(row: state::AgentRow) -> NodeManagedAgent {
    let running = pid_running(row.pid);
    NodeManagedAgent {
        id: row.id,
        track: row.track,
        pid: row.pid,
        process_state: if running { "running" } else { "exited" }.to_string(),
        stale: !running,
        started_at_unix: row.started_at,
        command: row.command,
        cwd: row.cwd.map(|path| path_display(&path)),
        stdout_log_path: row.stdout_log_path.map(|path| path_display(&path)),
        stderr_log_path: row.stderr_log_path.map(|path| path_display(&path)),
    }
}

fn node_managed_session(context: agents::TerminalAgentContext) -> NodeManagedSession {
    NodeManagedSession {
        agent_id: context.id,
        track: context.track,
        session: context.session,
        pane: context.pane,
        target: context.target,
        started_at_unix: context.started_at,
        command: context.command,
    }
}

fn collect_queue_visibility(now: u64) -> NodeQueueVisibility {
    let mut issues = Vec::new();
    let tabs = queue_tabs(now, &mut issues);
    let active_tab_id = tabs
        .iter()
        .find(|tab| tab.active)
        .map_or_else(|| "default".to_string(), |tab| tab.id.clone());
    let (active_run, active_items) = match state::load_web_queue() {
        Ok((run, items)) => (
            run.map(|run| node_queue_run(&run, now)),
            items
                .iter()
                .map(|item| node_queue_item(item, now))
                .collect::<Vec<_>>(),
        ),
        Err(err) => {
            issues.push(issue(
                "queue",
                NodeDataStatus::Partial,
                format!("active queue unavailable: {err:#}"),
            ));
            (None, Vec::new())
        }
    };
    let running = active_run
        .as_ref()
        .is_some_and(|run| matches!(run.status.as_str(), "running" | "waiting" | "starting"));
    let mut status = if issues.is_empty() {
        NodeDataStatus::Fresh
    } else {
        NodeDataStatus::Partial
    };
    let stale_queue_rows = usize::from(active_run.as_ref().is_some_and(|run| run.stale))
        + active_items.iter().filter(|item| item.stale).count()
        + tabs.iter().filter(|tab| tab.stale).count();
    if stale_queue_rows > 0 {
        issues.push(issue(
            "queue",
            NodeDataStatus::Stale,
            format!("stale queue rows present: {stale_queue_rows}"),
        ));
        status = status.merge(NodeDataStatus::Stale);
    }
    NodeQueueVisibility {
        status,
        count: active_items.len(),
        running,
        active_tab_id,
        active_run,
        active_items,
        tabs,
        issues,
    }
}

fn queue_tabs(now: u64, issues: &mut Vec<NodeSnapshotIssue>) -> Vec<NodeQueueTab> {
    let runs = state::load_web_queue_runs()
        .unwrap_or_default()
        .into_iter()
        .map(|(run, items)| (run.id.clone(), (run, items)))
        .collect::<HashMap<_, _>>();
    match state::load_web_queue_tabs() {
        Ok(tabs) => tabs
            .into_iter()
            .filter_map(|tab| {
                let run_entry = tab.run_id.as_ref().and_then(|run_id| runs.get(run_id));
                if run_entry.is_none() && !tab.active {
                    return None;
                }
                let status =
                    run_entry.map_or_else(|| "draft".to_string(), |(run, _)| run.status.to_string());
                let running = run_entry.is_some_and(|(run, _)| run.status.is_active());
                let stale = run_entry.is_some_and(|(run, _)| queue_run_stale(run, now));
                Some(NodeQueueTab {
                    id: tab.id,
                    label: tab.label,
                    run_id: tab.run_id,
                    status,
                    running,
                    count: run_entry.map_or(0, |(_, items)| items.len()),
                    active: tab.active,
                    is_default: tab.is_default,
                    updated_at_unix: tab.updated_at,
                    stale,
                })
            })
            .collect(),
        Err(err) => {
            issues.push(issue(
                "queue",
                NodeDataStatus::Partial,
                format!("queue tabs unavailable: {err:#}"),
            ));
            Vec::new()
        }
    }
}

fn node_queue_run(run: &state::QueueRunRow, now: u64) -> NodeQueueRun {
    NodeQueueRun {
        id: run.id.clone(),
        status: run.status.to_string(),
        execution_mode: run.execution_mode.to_string(),
        execution_host: run.execution_host.to_string(),
        selected_agent_command: run.selected_agent_command.clone(),
        selected_repo_root: run.selected_repo_root.clone(),
        selected_repo_name: run.selected_repo_name.clone(),
        remote_launcher: run.remote_launcher.clone(),
        remote_agent_local_proxy: run.remote_agent_local_proxy.clone(),
        remote_agent_remote_proxy: run.remote_agent_remote_proxy.clone(),
        current_index: run.current_index,
        stop_requested: run.stop_requested,
        message: run.message.clone(),
        created_at_unix: run.created_at,
        updated_at_unix: run.updated_at,
        stale: queue_run_stale(run, now),
    }
}

fn node_queue_item(item: &state::QueueItemRow, now: u64) -> NodeQueueItem {
    NodeQueueItem {
        id: item.id.clone(),
        run_id: item.run_id.clone(),
        position: item.position,
        slug: item.slug.clone(),
        status: item.status.to_string(),
        execution_host: item.execution_host.to_string(),
        agent_command: item.agent_command.clone(),
        agent_id: item.agent_id.clone(),
        repo_root: item.repo_root.clone(),
        repo_name: item.repo_name.clone(),
        remote_launcher: item.remote_launcher.clone(),
        remote_agent_local_proxy: item.remote_agent_local_proxy.clone(),
        remote_agent_remote_proxy: item.remote_agent_remote_proxy.clone(),
        attempts: item.attempts,
        recovery_attempts: item.recovery_attempts,
        next_attempt_at_unix: item.next_attempt_at,
        started_at_unix: item.started_at,
        updated_at_unix: item.updated_at,
        stale: queue_item_stale(item, now),
    }
}

fn queue_run_stale(run: &state::QueueRunRow, now: u64) -> bool {
    run.status.is_active() && now.saturating_sub(run.updated_at) > QUEUE_STALE_SECONDS
}

fn queue_item_stale(item: &state::QueueItemRow, now: u64) -> bool {
    item.status.is_active() && now.saturating_sub(item.updated_at) > QUEUE_STALE_SECONDS
}

fn collect_resource_snapshot() -> NodeResourceSnapshot {
    let mut issues = Vec::new();
    let cpu = collect_cpu().unwrap_or_else(|err| {
        issues.push(issue("resources.cpu", NodeDataStatus::Partial, err));
        None
    });
    let load = collect_load().unwrap_or_else(|err| {
        issues.push(issue("resources.load", NodeDataStatus::Partial, err));
        None
    });
    let (memory, swap) = collect_memory_swap().unwrap_or_else(|err| {
        issues.push(issue("resources.memory", NodeDataStatus::Partial, err));
        (None, None)
    });
    let disk = collect_disk().unwrap_or_else(|err| {
        issues.push(issue("resources.disk", NodeDataStatus::Partial, err));
        None
    });
    let pids = collect_pids().unwrap_or_else(|err| {
        issues.push(issue("resources.pids", NodeDataStatus::Partial, err));
        None
    });
    let io = collect_io().unwrap_or_else(|err| {
        issues.push(issue("resources.io", NodeDataStatus::Partial, err));
        None
    });
    let network = collect_network().unwrap_or_else(|err| {
        issues.push(issue("resources.network", NodeDataStatus::Partial, err));
        None
    });
    let present = [
        cpu.is_some(),
        load.is_some(),
        memory.is_some(),
        swap.is_some(),
        disk.is_some(),
        pids.is_some(),
        io.is_some(),
        network.is_some(),
    ];
    let status = if present.iter().all(|present| !present) {
        NodeDataStatus::Unavailable
    } else if issues.is_empty() {
        NodeDataStatus::Fresh
    } else {
        NodeDataStatus::Partial
    };
    NodeResourceSnapshot {
        status,
        cpu,
        load,
        memory,
        swap,
        disk,
        pids,
        io,
        network,
        issues,
    }
}

fn collect_cpu() -> std::result::Result<Option<NodeCpuSnapshot>, String> {
    let raw = read_required("/proc/stat")?;
    let cpu_line = raw
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(|| "missing aggregate cpu line in /proc/stat".to_string())?;
    let values = cpu_line
        .split_whitespace()
        .skip(1)
        .filter_map(|value| value.parse::<u64>().ok())
        .collect::<Vec<_>>();
    if values.len() < 4 {
        return Err("aggregate cpu line has too few counters".to_string());
    }
    let logical_cpus = raw
        .lines()
        .filter(|line| line.strip_prefix("cpu").is_some_and(cpu_line_has_index))
        .count();
    let total_jiffies = values.iter().sum();
    let idle_jiffies = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    Ok(Some(NodeCpuSnapshot {
        logical_cpus,
        total_jiffies,
        idle_jiffies,
        busy_jiffies: total_jiffies.saturating_sub(idle_jiffies),
    }))
}

fn cpu_line_has_index(rest: &str) -> bool {
    rest.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

fn collect_load() -> std::result::Result<Option<NodeLoadSnapshot>, String> {
    let raw = read_required("/proc/loadavg")?;
    let fields = raw.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 3 {
        return Err("/proc/loadavg has too few fields".to_string());
    }
    let process_counts = fields.get(3).and_then(|field| field.split_once('/'));
    Ok(Some(NodeLoadSnapshot {
        one: parse_f64(fields[0], "load one")?,
        five: parse_f64(fields[1], "load five")?,
        fifteen: parse_f64(fields[2], "load fifteen")?,
        running_processes: process_counts.and_then(|(running, _)| running.parse().ok()),
        total_processes: process_counts.and_then(|(_, total)| total.parse().ok()),
        last_pid: fields.get(4).and_then(|pid| pid.parse().ok()),
    }))
}

fn collect_memory_swap(
) -> std::result::Result<(Option<NodeMemorySnapshot>, Option<NodeSwapSnapshot>), String> {
    let raw = read_required("/proc/meminfo")?;
    let values = parse_meminfo(&raw);
    let total = kb_to_bytes(required_meminfo(&values, "MemTotal")?);
    let available = kb_to_bytes(required_meminfo(&values, "MemAvailable")?);
    let free = kb_to_bytes(required_meminfo(&values, "MemFree")?);
    let swap_total = kb_to_bytes(values.get("SwapTotal").copied().unwrap_or(0));
    let swap_free = kb_to_bytes(values.get("SwapFree").copied().unwrap_or(0));
    Ok((
        Some(NodeMemorySnapshot {
            total_bytes: total,
            available_bytes: available,
            free_bytes: free,
            used_bytes: total.saturating_sub(available),
        }),
        Some(NodeSwapSnapshot {
            total_bytes: swap_total,
            free_bytes: swap_free,
            used_bytes: swap_total.saturating_sub(swap_free),
        }),
    ))
}

fn parse_meminfo(raw: &str) -> HashMap<String, u64> {
    raw.lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            let value = rest.split_whitespace().next()?.parse().ok()?;
            Some((key.to_string(), value))
        })
        .collect()
}

fn required_meminfo(values: &HashMap<String, u64>, key: &str) -> std::result::Result<u64, String> {
    values
        .get(key)
        .copied()
        .ok_or_else(|| format!("missing {key} in /proc/meminfo"))
}

fn collect_disk() -> std::result::Result<Option<NodeDiskSnapshot>, String> {
    Ok(Some(NodeDiskSnapshot {
        root: disk_usage("/")?,
    }))
}

fn disk_usage(path: &str) -> std::result::Result<NodeDiskUsage, String> {
    let path_c = CString::new(path).map_err(|err| err.to_string())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path_c is a valid nul-terminated string and stats points to writable memory.
    let result = unsafe { libc::statvfs(path_c.as_ptr(), stats.as_mut_ptr()) };
    if result != 0 {
        return Err(format!("statvfs({path}) failed: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: statvfs returned success and initialized the stats structure.
    let stats = unsafe { stats.assume_init() };
    let block_size = stats.f_frsize.max(stats.f_bsize);
    let total = stats.f_blocks.saturating_mul(block_size);
    let free = stats.f_bfree.saturating_mul(block_size);
    let available = stats.f_bavail.saturating_mul(block_size);
    Ok(NodeDiskUsage {
        mount_point: path.to_string(),
        total_bytes: total,
        available_bytes: available,
        free_bytes: free,
        used_bytes: total.saturating_sub(free),
    })
}

fn collect_pids() -> std::result::Result<Option<NodePidSnapshot>, String> {
    let count = fs::read_dir("/proc")
        .map_err(|err| err.to_string())?
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().parse::<u32>().is_ok())
        .count();
    let pid_max = read_trimmed("/proc/sys/kernel/pid_max").and_then(|value| value.parse().ok());
    Ok(Some(NodePidSnapshot { count, pid_max }))
}

fn collect_io() -> std::result::Result<Option<NodeIoSnapshot>, String> {
    let raw = read_required("/proc/diskstats")?;
    let mut snapshot = NodeIoSnapshot::default();
    for line in raw.lines() {
        if let Some(device) = parse_diskstats_line(line) {
            snapshot.device_count += 1;
            snapshot.read_ios = snapshot.read_ios.saturating_add(device.read_ios);
            snapshot.write_ios = snapshot.write_ios.saturating_add(device.write_ios);
            snapshot.sectors_read = snapshot.sectors_read.saturating_add(device.sectors_read);
            snapshot.sectors_written = snapshot.sectors_written.saturating_add(device.sectors_written);
            snapshot.io_in_progress = snapshot.io_in_progress.saturating_add(device.io_in_progress);
        }
    }
    Ok(Some(snapshot))
}

#[derive(Clone, Copy)]
struct DiskstatsLine {
    read_ios: u64,
    write_ios: u64,
    sectors_read: u64,
    sectors_written: u64,
    io_in_progress: u64,
}

fn parse_diskstats_line(line: &str) -> Option<DiskstatsLine> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 14 || ignored_disk_device(fields[2]) {
        return None;
    }
    Some(DiskstatsLine {
        read_ios: fields.get(3)?.parse().ok()?,
        sectors_read: fields.get(5)?.parse().ok()?,
        write_ios: fields.get(7)?.parse().ok()?,
        sectors_written: fields.get(9)?.parse().ok()?,
        io_in_progress: fields.get(11)?.parse().ok()?,
    })
}

fn ignored_disk_device(name: &str) -> bool {
    name.starts_with("loop") || name.starts_with("ram")
}

fn collect_network() -> std::result::Result<Option<NodeNetworkSnapshot>, String> {
    let raw = read_required("/proc/net/dev")?;
    let interfaces = raw.lines().skip(2).filter_map(parse_netdev_line).collect::<Vec<_>>();
    let mut snapshot = NodeNetworkSnapshot {
        interface_count: interfaces.len(),
        rx_bytes: 0,
        rx_packets: 0,
        rx_errors: 0,
        rx_dropped: 0,
        tx_bytes: 0,
        tx_packets: 0,
        tx_errors: 0,
        tx_dropped: 0,
        interfaces,
    };
    for iface in &snapshot.interfaces {
        snapshot.rx_bytes = snapshot.rx_bytes.saturating_add(iface.rx_bytes);
        snapshot.rx_packets = snapshot.rx_packets.saturating_add(iface.rx_packets);
        snapshot.tx_bytes = snapshot.tx_bytes.saturating_add(iface.tx_bytes);
        snapshot.tx_packets = snapshot.tx_packets.saturating_add(iface.tx_packets);
    }
    for line in raw.lines().skip(2) {
        let Some((_, rest)) = line.split_once(':') else {
            continue;
        };
        let fields = rest.split_whitespace().collect::<Vec<_>>();
        snapshot.rx_errors = snapshot.rx_errors.saturating_add(parse_field(&fields, 2));
        snapshot.rx_dropped = snapshot.rx_dropped.saturating_add(parse_field(&fields, 3));
        snapshot.tx_errors = snapshot.tx_errors.saturating_add(parse_field(&fields, 10));
        snapshot.tx_dropped = snapshot.tx_dropped.saturating_add(parse_field(&fields, 11));
    }
    Ok(Some(snapshot))
}

fn parse_netdev_line(line: &str) -> Option<NodeNetworkInterface> {
    let (name, rest) = line.split_once(':')?;
    let fields = rest.split_whitespace().collect::<Vec<_>>();
    Some(NodeNetworkInterface {
        name: name.trim().to_string(),
        rx_bytes: parse_field(&fields, 0),
        rx_packets: parse_field(&fields, 1),
        tx_bytes: parse_field(&fields, 8),
        tx_packets: parse_field(&fields, 9),
    })
}

fn parse_field(fields: &[&str], index: usize) -> u64 {
    fields.get(index).and_then(|value| value.parse().ok()).unwrap_or(0)
}

fn collect_proxy_snapshot(queue: &NodeQueueVisibility) -> NodeProxySnapshot {
    let mut issues = Vec::new();
    let listeners = read_listening_tcp_ports().unwrap_or_else(|err| {
        issues.push(issue(
            "proxy",
            NodeDataStatus::Partial,
            format!("tcp listener state unavailable: {err:#}"),
        ));
        BTreeSet::new()
    });
    let env = proxy_env();
    let port_forwards = queue_port_forwards(queue, &listeners);
    let status = if !issues.is_empty() {
        NodeDataStatus::Partial
    } else if port_forwards.iter().any(|forward| forward.stale) {
        NodeDataStatus::Stale
    } else {
        NodeDataStatus::Fresh
    };
    NodeProxySnapshot {
        status,
        env,
        port_forwards,
        issues,
    }
}

fn proxy_env() -> Vec<NodeProxyEnv> {
    [
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "no_proxy",
    ]
    .into_iter()
    .filter_map(|name| {
        let value = env::var(name).ok()?;
        (!value.trim().is_empty()).then(|| NodeProxyEnv {
            name: name.to_string(),
            value: redact_proxy_value(&value),
        })
    })
    .collect()
}

fn queue_port_forwards(queue: &NodeQueueVisibility, listeners: &BTreeSet<u16>) -> Vec<NodePortForward> {
    let mut records = BTreeMap::new();
    if let Some(run) = &queue.active_run {
        push_proxy(
            &mut records,
            "active-run",
            "local-proxy",
            run.remote_agent_local_proxy.as_deref(),
            run.stale,
        );
        push_proxy(
            &mut records,
            "active-run",
            "remote-proxy",
            run.remote_agent_remote_proxy.as_deref(),
            run.stale,
        );
    }
    for item in &queue.active_items {
        let source = format!("item:{}", item.id);
        push_proxy(
            &mut records,
            &source,
            "local-proxy",
            item.remote_agent_local_proxy.as_deref(),
            item.stale,
        );
        push_proxy(
            &mut records,
            &source,
            "remote-proxy",
            item.remote_agent_remote_proxy.as_deref(),
            item.stale,
        );
    }
    records
        .into_values()
        .map(|mut record| {
            record.observed_state = observed_endpoint_state(&record.endpoint, listeners);
            record
        })
        .collect()
}

fn push_proxy(
    records: &mut BTreeMap<String, NodePortForward>,
    source: &str,
    direction: &str,
    endpoint: Option<&str>,
    stale: bool,
) {
    let Some(endpoint) = endpoint.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let key = format!("{source}\t{direction}\t{endpoint}");
    records.entry(key).or_insert_with(|| NodePortForward {
        source: source.to_string(),
        direction: direction.to_string(),
        endpoint: endpoint.to_string(),
        observed_state: "unknown".to_string(),
        stale,
    });
}

fn read_listening_tcp_ports() -> Result<BTreeSet<u16>> {
    let mut ports = BTreeSet::new();
    read_tcp_listeners("/proc/net/tcp", &mut ports)?;
    read_tcp_listeners("/proc/net/tcp6", &mut ports)?;
    Ok(ports)
}

fn read_tcp_listeners(path: &str, ports: &mut BTreeSet<u16>) -> Result<()> {
    let raw = fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
    for line in raw.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.get(3).copied() != Some("0A") {
            continue;
        }
        if let Some(port) = fields.get(1).and_then(|local| tcp_local_port(local)) {
            ports.insert(port);
        }
    }
    Ok(())
}

fn tcp_local_port(local: &str) -> Option<u16> {
    let (_, port_hex) = local.rsplit_once(':')?;
    u16::from_str_radix(port_hex, 16).ok()
}

fn observed_endpoint_state(endpoint: &str, listeners: &BTreeSet<u16>) -> String {
    endpoint_port(endpoint).map_or_else(
        || "unknown".to_string(),
        |port| {
            if listeners.contains(&port) {
                "listening".to_string()
            } else {
                "not-listening".to_string()
            }
        },
    )
}

fn endpoint_port(endpoint: &str) -> Option<u16> {
    let without_scheme = endpoint.split("://").nth(1).unwrap_or(endpoint);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    let (_, port) = host_port.rsplit_once(':')?;
    port.parse().ok()
}

fn issue(block: &str, status: NodeDataStatus, message: impl Into<String>) -> NodeSnapshotIssue {
    NodeSnapshotIssue {
        block: block.to_string(),
        status,
        message: message.into(),
    }
}

fn state_dir() -> PathBuf {
    env::var("QCOLD_STATE_DIR")
        .ok()
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/state/qcold"))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

fn legacy_agents_path() -> PathBuf {
    state_dir().join("agents.tsv")
}

fn pid_running(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

fn path_display(path: &Path) -> String {
    path.display().to_string()
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_required(path: &str) -> std::result::Result<String, String> {
    fs::read_to_string(path).map_err(|err| format!("failed to read {path}: {err}"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn parse_uptime_ms(raw: &str) -> Option<u64> {
    let seconds = raw.split_whitespace().next()?.parse::<f64>().ok()?;
    if seconds.is_sign_negative() || !seconds.is_finite() {
        return None;
    }
    u64::try_from(Duration::from_secs_f64(seconds).as_millis()).ok()
}

fn parse_f64(value: &str, label: &str) -> std::result::Result<f64, String> {
    value.parse().map_err(|err| format!("invalid {label}: {err}"))
}

fn kb_to_bytes(value: u64) -> u64 {
    value.saturating_mul(1024)
}

fn redact_proxy_value(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    let Some((auth, host)) = rest.split_once('@') else {
        return value.to_string();
    };
    if auth.is_empty() {
        value.to_string()
    } else {
        format!("{scheme}://[redacted]@{host}")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn endpoint_port_handles_urls_and_host_ports() {
        assert_eq!(endpoint_port("127.0.0.1:18100"), Some(18100));
        assert_eq!(endpoint_port("http://127.0.0.1:3128/proxy"), Some(3128));
        assert_eq!(endpoint_port("not-a-port"), None);
    }

    #[test]
    fn proxy_redaction_keeps_host_and_hides_credentials() {
        assert_eq!(
            redact_proxy_value("http://user:secret@127.0.0.1:3128"),
            "http://[redacted]@127.0.0.1:3128"
        );
        assert_eq!(redact_proxy_value("http://127.0.0.1:3128"), "http://127.0.0.1:3128");
    }

    #[test]
    fn meminfo_parser_reads_kilobyte_fields() {
        let parsed = parse_meminfo("MemTotal:       100 kB\nMemAvailable:    80 kB\n");
        assert_eq!(parsed.get("MemTotal"), Some(&100));
        assert_eq!(parsed.get("MemAvailable"), Some(&80));
    }

    #[test]
    fn uptime_parser_returns_monotonic_milliseconds() {
        assert_eq!(parse_uptime_ms("12.34 99.00"), Some(12_340));
    }

    #[test]
    fn data_status_merge_keeps_worst_status() {
        assert_eq!(
            NodeDataStatus::Fresh.merge(NodeDataStatus::Stale),
            NodeDataStatus::Stale
        );
        assert_eq!(
            NodeDataStatus::Partial.merge(NodeDataStatus::Stale),
            NodeDataStatus::Partial
        );
    }
}
