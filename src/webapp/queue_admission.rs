const QUEUE_ADMISSION_HISTORY_SECONDS: u64 = 60 * 60;
const QUEUE_ADMISSION_RETRY_SECONDS: u64 = 60;
const QUEUE_ADMISSION_DEFAULT_SOFT_TASKS: usize = 8;
const QUEUE_ADMISSION_DEFAULT_HARD_TASKS: usize = 12;
const QUEUE_ADMISSION_DEFAULT_HEAVY_TASKS: usize = 2;
const QUEUE_ADMISSION_REMOTE_RESOURCE_TIMEOUT: &str = "15s";
const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;
const MIN_MEMORY_SAFETY_FLOOR_GIB: u64 = 2;
const MAX_MEMORY_SAFETY_FLOOR_GIB: u64 = 8;

#[derive(Clone, Copy, Debug)]
struct QueueAdmissionPolicy {
    soft_task_limit: usize,
    hard_task_limit: usize,
    heavy_task_limit: usize,
    retry_seconds: u64,
    history_seconds: u64,
}

impl Default for QueueAdmissionPolicy {
    fn default() -> Self {
        Self {
            soft_task_limit: QUEUE_ADMISSION_DEFAULT_SOFT_TASKS,
            hard_task_limit: QUEUE_ADMISSION_DEFAULT_HARD_TASKS,
            heavy_task_limit: QUEUE_ADMISSION_DEFAULT_HEAVY_TASKS,
            retry_seconds: QUEUE_ADMISSION_RETRY_SECONDS,
            history_seconds: QUEUE_ADMISSION_HISTORY_SECONDS,
        }
    }
}

impl QueueAdmissionPolicy {
    fn from_env() -> Self {
        let defaults = Self::default();
        let soft_task_limit = env_usize("QCOLD_QUEUE_ADMISSION_SOFT_MAX_TASKS")
            .unwrap_or(defaults.soft_task_limit);
        let hard_task_limit = env_usize("QCOLD_QUEUE_ADMISSION_HARD_MAX_TASKS")
            .unwrap_or(defaults.hard_task_limit)
            .max(soft_task_limit);
        Self {
            soft_task_limit,
            hard_task_limit,
            heavy_task_limit: env_usize("QCOLD_QUEUE_ADMISSION_HEAVY_MAX_TASKS")
                .unwrap_or(defaults.heavy_task_limit),
            retry_seconds: env_u64("QCOLD_QUEUE_ADMISSION_RETRY_SECONDS")
                .unwrap_or(defaults.retry_seconds),
            history_seconds: env_u64("QCOLD_QUEUE_ADMISSION_HISTORY_SECONDS")
                .unwrap_or(defaults.history_seconds),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct QueueResourceReservation {
    tasks: usize,
    heavy_tasks: usize,
    cpu_millis: u64,
    memory_bytes: u64,
}

impl QueueResourceReservation {
    fn for_task_class(task_class: state::QueueTaskClass) -> Self {
        match task_class {
            state::QueueTaskClass::Cheap => Self {
                tasks: 1,
                heavy_tasks: 0,
                cpu_millis: 250,
                memory_bytes: 2 * BYTES_PER_GIB,
            },
            state::QueueTaskClass::Mid => Self {
                tasks: 1,
                heavy_tasks: 0,
                cpu_millis: 500,
                memory_bytes: 8 * BYTES_PER_GIB,
            },
            state::QueueTaskClass::Heavy => Self {
                tasks: 1,
                heavy_tasks: 1,
                cpu_millis: 2_000,
                memory_bytes: 32 * BYTES_PER_GIB,
            },
        }
    }

    fn reserve(&mut self, task_class: state::QueueTaskClass) {
        let next = Self::for_task_class(task_class);
        self.tasks = self.tasks.saturating_add(next.tasks);
        self.heavy_tasks = self.heavy_tasks.saturating_add(next.heavy_tasks);
        self.cpu_millis = self.cpu_millis.saturating_add(next.cpu_millis);
        self.memory_bytes = self.memory_bytes.saturating_add(next.memory_bytes);
    }
}

#[derive(Clone, Copy, Debug)]
struct QueueAdmissionHostResources {
    logical_cpus: usize,
    load_one: Option<f64>,
    memory_total_bytes: Option<u64>,
    memory_available_bytes: Option<u64>,
}

impl Default for QueueAdmissionHostResources {
    fn default() -> Self {
        Self {
            logical_cpus: 8,
            load_one: None,
            memory_total_bytes: Some(128 * BYTES_PER_GIB),
            memory_available_bytes: None,
        }
    }
}

impl QueueAdmissionHostResources {
    fn from_node_snapshot(snapshot: &crate::node_agent::NodeSnapshot) -> Self {
        let cpu = snapshot.resources.cpu.as_ref();
        let memory = snapshot.resources.memory.as_ref();
        Self {
            logical_cpus: cpu
                .map(|cpu| cpu.logical_cpus)
                .filter(|cpus| *cpus > 0)
                .unwrap_or(8),
            load_one: snapshot.resources.load.as_ref().map(|load| load.one),
            memory_total_bytes: memory.map(|memory| memory.total_bytes),
            memory_available_bytes: memory.map(|memory| memory.available_bytes),
        }
    }

    fn to_sample(
        self,
        sampled_at: u64,
        reservations: QueueResourceReservation,
    ) -> state::QueueResourceSampleRow {
        state::QueueResourceSampleRow {
            sampled_at,
            logical_cpus: Some(i64::try_from(self.logical_cpus).unwrap_or(i64::MAX)),
            load_one_milli: self
                .load_one
                .map(load_to_milli)
                .map(|load| i64::try_from(load).unwrap_or(i64::MAX)),
            memory_total_bytes: self
                .memory_total_bytes
                .map(|value| i64::try_from(value).unwrap_or(i64::MAX)),
            memory_available_bytes: self
                .memory_available_bytes
                .map(|value| i64::try_from(value).unwrap_or(i64::MAX)),
            reserved_tasks: i64::try_from(reservations.tasks).unwrap_or(i64::MAX),
            reserved_heavy_tasks: i64::try_from(reservations.heavy_tasks).unwrap_or(i64::MAX),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct QueueAdmissionHistory {
    peak_load_one_milli: Option<u64>,
    min_memory_available_bytes: Option<u64>,
    peak_reserved_tasks: usize,
    peak_reserved_heavy_tasks: usize,
}

impl QueueAdmissionHistory {
    fn from_samples(samples: &[state::QueueResourceSampleRow]) -> Self {
        let mut history = Self::default();
        for sample in samples {
            if let Some(load) = sample.load_one_milli.and_then(non_negative_i64) {
                history.peak_load_one_milli =
                    Some(history.peak_load_one_milli.map_or(load, |peak| peak.max(load)));
            }
            if let Some(memory) = sample.memory_available_bytes.and_then(non_negative_i64) {
                history.min_memory_available_bytes = Some(
                    history
                        .min_memory_available_bytes
                        .map_or(memory, |minimum| minimum.min(memory)),
                );
            }
            if let Some(tasks) = non_negative_i64(sample.reserved_tasks) {
                history.peak_reserved_tasks = history
                    .peak_reserved_tasks
                    .max(usize::try_from(tasks).unwrap_or(usize::MAX));
            }
            if let Some(heavy) = non_negative_i64(sample.reserved_heavy_tasks) {
                history.peak_reserved_heavy_tasks = history
                    .peak_reserved_heavy_tasks
                    .max(usize::try_from(heavy).unwrap_or(usize::MAX));
            }
        }
        history
    }
}

#[derive(Clone, Debug)]
struct QueueAdmissionContext {
    policy: QueueAdmissionPolicy,
    now: u64,
    reservations: QueueResourceReservation,
    host: QueueAdmissionHostResources,
    history: QueueAdmissionHistory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QueueAdmissionWait {
    reason: String,
    next_retry_at: u64,
}

#[derive(Clone, Debug, Default)]
struct QueueAdmissionPlan {
    admitted: Vec<state::QueueItemRow>,
    waiting: Vec<(state::QueueItemRow, QueueAdmissionWait)>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum QueueAdmissionScope {
    Local,
    RemoteNative { launcher: String },
    RemoteNativeMissingLauncher,
}

impl QueueAdmissionScope {
    fn for_item(item: &state::QueueItemRow) -> Self {
        if item.execution_host.is_remote_native() {
            return item
                .remote_launcher
                .as_deref()
                .map(str::trim)
                .filter(|launcher| !launcher.is_empty())
                .map_or(Self::RemoteNativeMissingLauncher, |launcher| Self::RemoteNative {
                    launcher: launcher.to_string(),
                });
        }
        Self::Local
    }
}

fn select_queue_admission(
    ready: Vec<state::QueueItemRow>,
    mut context: QueueAdmissionContext,
) -> QueueAdmissionPlan {
    let mut plan = QueueAdmissionPlan::default();
    for item in ready {
        if let Some(wait) = queue_admission_wait(&context, item.task_class) {
            plan.waiting.push((item, wait));
        } else {
            context.reservations.reserve(item.task_class);
            plan.admitted.push(item);
        }
    }
    plan
}

fn queue_admission_wait(
    context: &QueueAdmissionContext,
    task_class: state::QueueTaskClass,
) -> Option<QueueAdmissionWait> {
    let requested = QueueResourceReservation::for_task_class(task_class);
    let post_tasks = context.reservations.tasks.saturating_add(requested.tasks);
    let post_heavy = context
        .reservations
        .heavy_tasks
        .saturating_add(requested.heavy_tasks);
    let retry_at = context.now.saturating_add(context.policy.retry_seconds);

    if post_tasks > context.policy.hard_task_limit {
        return Some(wait_reason(
            format!(
                "hard task limit {} would be exceeded ({post_tasks} requested)",
                context.policy.hard_task_limit
            ),
            retry_at,
        ));
    }
    if post_heavy > context.policy.heavy_task_limit {
        return Some(wait_reason(
            format!(
                "heavy task limit {} would be exceeded ({post_heavy} requested)",
                context.policy.heavy_task_limit
            ),
            retry_at,
        ));
    }
    if current_load_too_high(context) {
        return Some(wait_reason("host load is above admission threshold", retry_at));
    }
    if memory_too_low(context, requested.memory_bytes) {
        return Some(wait_reason("available memory is below requested reservation", retry_at));
    }
    if post_tasks > context.policy.soft_task_limit && recent_resource_pressure(context) {
        return Some(wait_reason(
            format!(
                "soft task limit {} would be exceeded under recent resource pressure",
                context.policy.soft_task_limit
            ),
            retry_at,
        ));
    }
    let post_cpu = context
        .reservations
        .cpu_millis
        .saturating_add(requested.cpu_millis);
    let host_cpu = u64::try_from(context.host.logical_cpus)
        .unwrap_or(u64::MAX / 1000)
        .saturating_mul(1000);
    if post_cpu > host_cpu {
        return Some(wait_reason(
            format!("reserved CPU would exceed {} logical core(s)", context.host.logical_cpus),
            retry_at,
        ));
    }
    None
}

fn wait_reason(reason: impl Into<String>, next_retry_at: u64) -> QueueAdmissionWait {
    QueueAdmissionWait {
        reason: reason.into(),
        next_retry_at,
    }
}

fn current_load_too_high(context: &QueueAdmissionContext) -> bool {
    context.host.load_one.is_some_and(|load| {
        load_to_milli(load) >= load_threshold_milli(context.host.logical_cpus, 1_250)
    })
}

fn memory_too_low(context: &QueueAdmissionContext, requested_memory_bytes: u64) -> bool {
    let floor = memory_safety_floor(context.host);
    let post_reserved = context
        .reservations
        .memory_bytes
        .saturating_add(requested_memory_bytes);
    if context
        .host
        .memory_total_bytes
        .is_some_and(|total| post_reserved.saturating_add(floor) > total)
    {
        return true;
    }
    context
        .host
        .memory_available_bytes
        .is_some_and(|available| available < requested_memory_bytes.saturating_add(floor))
}

fn recent_resource_pressure(context: &QueueAdmissionContext) -> bool {
    context
        .history
        .peak_load_one_milli
        .is_some_and(|load| load >= load_threshold_milli(context.host.logical_cpus, 900))
        || context
            .history
            .min_memory_available_bytes
            .is_some_and(|available| available < memory_safety_floor(context.host))
        || context.history.peak_reserved_tasks >= context.policy.hard_task_limit
        || context.history.peak_reserved_heavy_tasks >= context.policy.heavy_task_limit
}

fn memory_safety_floor(host: QueueAdmissionHostResources) -> u64 {
    host.memory_total_bytes.map_or(MAX_MEMORY_SAFETY_FLOOR_GIB * BYTES_PER_GIB, |total| {
        (total / 8).clamp(
            MIN_MEMORY_SAFETY_FLOOR_GIB * BYTES_PER_GIB,
            MAX_MEMORY_SAFETY_FLOOR_GIB * BYTES_PER_GIB,
        )
    })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "load averages are non-negative host samples stored as coarse milli-units"
)]
fn load_to_milli(load: f64) -> u64 {
    (load.max(0.0) * 1000.0).round() as u64
}

fn load_threshold_milli(logical_cpus: usize, per_cpu_milli: u64) -> u64 {
    u64::try_from(logical_cpus.max(1))
        .unwrap_or(u64::MAX / per_cpu_milli.max(1))
        .saturating_mul(per_cpu_milli)
}

fn queue_resource_reservations_for_scope(
    items: &[state::QueueItemRow],
    scope: &QueueAdmissionScope,
) -> QueueResourceReservation {
    let mut reservations = QueueResourceReservation::default();
    for item in items {
        if &QueueAdmissionScope::for_item(item) == scope
            && (item.status.has_executor_session()
                || queue_item_worker_active(&item.run_id, &item.id))
        {
            reservations.reserve(item.task_class);
        }
    }
    reservations
}

fn queue_admission_context(
    scope: &QueueAdmissionScope,
    items: &[state::QueueItemRow],
) -> Result<QueueAdmissionContext> {
    let policy = QueueAdmissionPolicy::from_env();
    let now = unix_now();
    let reservations = queue_resource_reservations_for_scope(items, scope);
    let host = match scope {
        QueueAdmissionScope::Local => {
            QueueAdmissionHostResources::from_node_snapshot(&crate::node_agent::collect_snapshot())
        }
        QueueAdmissionScope::RemoteNative { launcher } => remote_admission_host_resources(launcher)?,
        QueueAdmissionScope::RemoteNativeMissingLauncher => {
            bail!("remote-native queue item requires remote_launcher or selected_remote_launcher")
        }
    };
    let retain_since = now.saturating_sub(policy.history_seconds);
    let history = if matches!(scope, QueueAdmissionScope::Local) {
        state::record_queue_resource_sample(&host.to_sample(now, reservations), retain_since)?;
        let samples = state::load_queue_resource_samples_since(retain_since)?;
        QueueAdmissionHistory::from_samples(&samples)
    } else {
        QueueAdmissionHistory::default()
    };
    Ok(QueueAdmissionContext {
        policy,
        now,
        reservations,
        host,
        history,
    })
}

fn apply_queue_admission(ready: Vec<state::QueueItemRow>) -> Result<QueueAdmissionPlan> {
    if ready.is_empty() {
        return Ok(QueueAdmissionPlan::default());
    }
    let all_items = state::load_web_queue_items()?;
    let mut ready_by_scope: BTreeMap<QueueAdmissionScope, Vec<state::QueueItemRow>> =
        BTreeMap::new();
    for item in ready {
        ready_by_scope
            .entry(QueueAdmissionScope::for_item(&item))
            .or_default()
            .push(item);
    }
    let mut plan = QueueAdmissionPlan::default();
    for (scope, items) in ready_by_scope {
        match queue_admission_context(&scope, &all_items) {
            Ok(context) => {
                let scope_plan = select_queue_admission(items, context);
                plan.admitted.extend(scope_plan.admitted);
                plan.waiting.extend(scope_plan.waiting);
            }
            Err(err)
                if matches!(
                    scope,
                    QueueAdmissionScope::RemoteNative { .. }
                        | QueueAdmissionScope::RemoteNativeMissingLauncher
                ) =>
            {
                let wait = QueueAdmissionWait {
                    reason: format!("remote resource sample unavailable: {err:#}"),
                    next_retry_at: unix_now()
                        .saturating_add(QueueAdmissionPolicy::from_env().retry_seconds),
                };
                plan.waiting
                    .extend(items.into_iter().map(|item| (item, wait.clone())));
            }
            Err(err) => return Err(err),
        }
    }
    update_queue_admission_waiting(&plan.waiting)?;
    Ok(plan)
}

fn update_queue_admission_waiting(
    waiting: &[(state::QueueItemRow, QueueAdmissionWait)],
) -> Result<()> {
    for (item, wait) in waiting {
        let message = format!(
            "admission waiting: {}; retry_at={}",
            wait.reason, wait.next_retry_at
        );
        state::update_web_queue_item(
            &item.run_id,
            &item.id,
            "waiting",
            &message,
            item.agent_id.as_deref(),
            item.attempts,
            Some(wait.next_retry_at),
        )?;
    }
    Ok(())
}

fn remote_admission_host_resources(launcher: &str) -> Result<QueueAdmissionHostResources> {
    let script = concat!(
        "printf 'cpus=%s\\n' ",
        "\"$(getconf _NPROCESSORS_ONLN 2>/dev/null || nproc 2>/dev/null || echo 8)\"; ",
        "awk '/^MemTotal:/ {print \"mem_total_kib=\"$2} ",
        "/^MemAvailable:/ {print \"mem_available_kib=\"$2}' /proc/meminfo; ",
        "awk '{print \"load_one=\"$1}' /proc/loadavg",
    );
    let output = Command::new("timeout")
        .arg(QUEUE_ADMISSION_REMOTE_RESOURCE_TIMEOUT)
        .arg(launcher)
        .arg("sh")
        .arg("-lc")
        .arg(script)
        .output()
        .with_context(|| format!("failed to collect remote admission resources through {launcher}"))?;
    if !output.status.success() {
        bail!(
            "remote admission resource command through {launcher} failed: {}",
            compact_remote_admission_output(&output.stdout, &output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_remote_admission_resources(&stdout)
        .with_context(|| format!("failed to parse remote admission resources from {launcher}"))
}

fn parse_remote_admission_resources(output: &str) -> Result<QueueAdmissionHostResources> {
    let mut logical_cpus = None;
    let mut load_one = None;
    let mut memory_total_bytes = None;
    let mut memory_available_bytes = None;

    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "cpus" => {
                logical_cpus = value.parse::<usize>().ok().filter(|cpus| *cpus > 0);
            }
            "load_one" => {
                load_one = value.parse::<f64>().ok();
            }
            "mem_total_kib" => {
                memory_total_bytes = value
                    .parse::<u64>()
                    .ok()
                    .and_then(|kib| kib.checked_mul(1024));
            }
            "mem_available_kib" => {
                memory_available_bytes = value
                    .parse::<u64>()
                    .ok()
                    .and_then(|kib| kib.checked_mul(1024));
            }
            _ => {}
        }
    }

    Ok(QueueAdmissionHostResources {
        logical_cpus: logical_cpus.context("missing logical CPU count")?,
        load_one,
        memory_total_bytes: Some(memory_total_bytes.context("missing MemTotal")?),
        memory_available_bytes: Some(memory_available_bytes.context("missing MemAvailable")?),
    })
}

fn compact_remote_admission_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let detail = format!("stdout={} stderr={}", stdout.trim(), stderr.trim());
    if detail.len() > QUEUE_PROCESS_OUTPUT_LIMIT {
        let end = detail
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= QUEUE_PROCESS_OUTPUT_LIMIT)
            .last()
            .unwrap_or(0);
        format!("{}...", &detail[..end])
    } else {
        detail
    }
}

fn env_usize(name: &str) -> Option<usize> {
    env::var(name).ok()?.trim().parse().ok()
}

fn env_u64(name: &str) -> Option<u64> {
    env::var(name).ok()?.trim().parse().ok()
}

fn non_negative_i64(value: i64) -> Option<u64> {
    u64::try_from(value).ok()
}

#[cfg(test)]
mod queue_admission_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn admission_hard_limit_keeps_excess_ready_items_waiting() {
        let context = admission_context(QueueResourceReservation::default(), healthy_host(), default_history());
        let plan = select_queue_admission(mid_items(13), context);

        assert_eq!(plan.admitted.len(), 12);
        assert_eq!(plan.waiting.len(), 1);
        assert!(plan.waiting[0].1.reason.contains("hard task limit 12"));
    }

    #[test]
    fn admission_heavy_limit_caps_heavy_fanout() {
        let context = admission_context(QueueResourceReservation::default(), healthy_host(), default_history());
        let plan = select_queue_admission(items_with_class(3, state::QueueTaskClass::Heavy), context);

        assert_eq!(plan.admitted.len(), 2);
        assert_eq!(plan.waiting.len(), 1);
        assert!(plan.waiting[0].1.reason.contains("heavy task limit 2"));
    }

    #[test]
    fn admission_soft_limit_waits_when_recent_history_is_pressured() {
        let mut reservations = QueueResourceReservation::default();
        for _ in 0..8 {
            reservations.reserve(state::QueueTaskClass::Mid);
        }
        let history = QueueAdmissionHistory {
            peak_load_one_milli: Some(7_500),
            ..QueueAdmissionHistory::default()
        };
        let context = admission_context(reservations, healthy_host(), history);
        let plan = select_queue_admission(mid_items(1), context);

        assert!(plan.admitted.is_empty());
        assert_eq!(plan.waiting.len(), 1);
        assert!(plan.waiting[0].1.reason.contains("soft task limit 8"));
    }

    #[test]
    fn admission_soft_limit_allows_burst_when_resources_are_healthy() {
        let mut reservations = QueueResourceReservation::default();
        for _ in 0..8 {
            reservations.reserve(state::QueueTaskClass::Mid);
        }
        let context = admission_context(reservations, healthy_host(), default_history());
        let plan = select_queue_admission(mid_items(1), context);

        assert_eq!(plan.admitted.len(), 1);
        assert!(plan.waiting.is_empty());
    }

    #[test]
    fn admission_memory_limit_includes_live_reservations() {
        let reservations = QueueResourceReservation {
            tasks: 1,
            memory_bytes: 120 * BYTES_PER_GIB,
            ..QueueResourceReservation::default()
        };
        let context = admission_context(reservations, healthy_host(), default_history());
        let plan = select_queue_admission(items_with_class(1, state::QueueTaskClass::Cheap), context);

        assert!(plan.admitted.is_empty());
        assert_eq!(plan.waiting.len(), 1);
        assert!(
            plan.waiting[0]
                .1
                .reason
                .contains("available memory is below requested reservation")
        );
    }

    #[test]
    fn admission_mid_task_can_run_on_16g_host_with_real_headroom() {
        let context = admission_context(
            QueueResourceReservation::default(),
            host_with_memory(16, 11),
            default_history(),
        );
        let plan = select_queue_admission(mid_items(1), context);

        assert_eq!(plan.admitted.len(), 1);
        assert!(plan.waiting.is_empty());
    }

    #[test]
    fn admission_mid_fanout_on_16g_host_keeps_next_item_waiting() {
        let context = admission_context(
            QueueResourceReservation::default(),
            host_with_memory(16, 11),
            default_history(),
        );
        let plan = select_queue_admission(mid_items(2), context);

        assert_eq!(plan.admitted.len(), 1);
        assert_eq!(plan.waiting.len(), 1);
        assert!(
            plan.waiting[0]
                .1
                .reason
                .contains("available memory is below requested reservation")
        );
    }

    #[test]
    fn admission_large_host_keeps_8g_safety_floor() {
        assert_eq!(
            memory_safety_floor(host_with_memory(128, 96)),
            8 * BYTES_PER_GIB
        );
    }

    #[test]
    fn admission_unknown_memory_total_uses_8g_safety_floor() {
        assert_eq!(
            memory_safety_floor(QueueAdmissionHostResources {
                memory_total_bytes: None,
                ..host_with_memory(16, 11)
            }),
            8 * BYTES_PER_GIB
        );
    }

    #[test]
    fn admission_scope_keys_remote_items_by_launcher() {
        let mut local = queue_item("local", state::QueueTaskClass::Mid);
        let mut remote_a = queue_item("remote-a", state::QueueTaskClass::Heavy);
        remote_a.execution_host = state::QueueExecutionHost::RemoteNative;
        remote_a.remote_launcher = Some(" remote-dev-env ".to_string());
        let mut remote_b = queue_item("remote-b", state::QueueTaskClass::Heavy);
        remote_b.execution_host = state::QueueExecutionHost::RemoteNative;
        remote_b.remote_launcher = Some("other-remote".to_string());
        let mut remote_missing = queue_item("remote-missing", state::QueueTaskClass::Heavy);
        remote_missing.execution_host = state::QueueExecutionHost::RemoteNative;
        local.remote_launcher = Some("ignored-for-local".to_string());

        assert_eq!(QueueAdmissionScope::for_item(&local), QueueAdmissionScope::Local);
        assert_eq!(
            QueueAdmissionScope::for_item(&remote_a),
            QueueAdmissionScope::RemoteNative {
                launcher: "remote-dev-env".to_string()
            }
        );
        assert_eq!(
            QueueAdmissionScope::for_item(&remote_b),
            QueueAdmissionScope::RemoteNative {
                launcher: "other-remote".to_string()
            }
        );
        assert_eq!(
            QueueAdmissionScope::for_item(&remote_missing),
            QueueAdmissionScope::RemoteNativeMissingLauncher
        );
    }

    #[test]
    fn admission_reservations_are_scoped_by_execution_host() {
        let mut local = queue_item("local", state::QueueTaskClass::Heavy);
        local.status = state::QueueItemStatus::Running;
        let mut remote_a = queue_item("remote-a", state::QueueTaskClass::Heavy);
        remote_a.status = state::QueueItemStatus::Running;
        remote_a.execution_host = state::QueueExecutionHost::RemoteNative;
        remote_a.remote_launcher = Some("remote-dev-env".to_string());
        let mut remote_b = queue_item("remote-b", state::QueueTaskClass::Heavy);
        remote_b.status = state::QueueItemStatus::Running;
        remote_b.execution_host = state::QueueExecutionHost::RemoteNative;
        remote_b.remote_launcher = Some("other-remote".to_string());
        let items = vec![local, remote_a, remote_b];

        assert_eq!(
            queue_resource_reservations_for_scope(&items, &QueueAdmissionScope::Local).heavy_tasks,
            1
        );
        assert_eq!(
            queue_resource_reservations_for_scope(
                &items,
                &QueueAdmissionScope::RemoteNative {
                    launcher: "remote-dev-env".to_string()
                }
            )
            .heavy_tasks,
            1
        );
        assert_eq!(
            queue_resource_reservations_for_scope(
                &items,
                &QueueAdmissionScope::RemoteNative {
                    launcher: "other-remote".to_string()
                }
            )
            .heavy_tasks,
            1
        );
    }

    #[test]
    fn admission_parses_remote_resource_snapshot() {
        let host = parse_remote_admission_resources(
            "cpus=32\nmem_total_kib=126877696\nmem_available_kib=89128960\nload_one=3.25\n",
        )
        .unwrap();

        assert_eq!(host.logical_cpus, 32);
        assert_eq!(host.load_one, Some(3.25));
        assert_eq!(host.memory_total_bytes, Some(126877696 * 1024));
        assert_eq!(host.memory_available_bytes, Some(89128960 * 1024));
    }

    fn admission_context(
        reservations: QueueResourceReservation,
        host: QueueAdmissionHostResources,
        history: QueueAdmissionHistory,
    ) -> QueueAdmissionContext {
        QueueAdmissionContext {
            policy: QueueAdmissionPolicy::default(),
            now: 1_000,
            reservations,
            host,
            history,
        }
    }

    fn host_with_memory(total_gib: u64, available_gib: u64) -> QueueAdmissionHostResources {
        QueueAdmissionHostResources {
            logical_cpus: 8,
            load_one: Some(2.0),
            memory_total_bytes: Some(total_gib * BYTES_PER_GIB),
            memory_available_bytes: Some(available_gib * BYTES_PER_GIB),
        }
    }

    fn healthy_host() -> QueueAdmissionHostResources {
        QueueAdmissionHostResources {
            logical_cpus: 8,
            load_one: Some(2.0),
            memory_total_bytes: Some(128 * BYTES_PER_GIB),
            memory_available_bytes: Some(96 * BYTES_PER_GIB),
        }
    }

    fn default_history() -> QueueAdmissionHistory {
        QueueAdmissionHistory::default()
    }

    fn mid_items(count: usize) -> Vec<state::QueueItemRow> {
        items_with_class(count, state::QueueTaskClass::Mid)
    }

    fn items_with_class(count: usize, task_class: state::QueueTaskClass) -> Vec<state::QueueItemRow> {
        (0..count)
            .map(|index| queue_item(&format!("item-{index}"), task_class))
            .collect()
    }

    fn queue_item(id: &str, task_class: state::QueueTaskClass) -> state::QueueItemRow {
        state::QueueItemRow {
            id: id.to_string(),
            run_id: "run".to_string(),
            position: 0,
            depends_on: Vec::new(),
            prompt: "prompt".to_string(),
            slug: id.to_string(),
            repo_root: None,
            repo_name: None,
            execution_host: state::QueueExecutionHost::Local,
            agent_command: "c1".to_string(),
            task_class,
            remote_launcher: None,
            remote_agent_local_proxy: None,
            remote_agent_remote_proxy: None,
            agent_id: None,
            status: state::QueueItemStatus::Pending,
            message: String::new(),
            attempts: 0,
            recovery_attempts: 0,
            next_attempt_at: None,
            started_at: 1,
            updated_at: 1,
        }
    }
}
