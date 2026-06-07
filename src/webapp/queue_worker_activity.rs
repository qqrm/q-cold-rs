#[derive(Default)]
struct QueueActivitySnapshot {
    local_active_items: HashSet<String>,
    lease_active_items: HashSet<String>,
}

fn queue_activity_snapshot(run_id: &str) -> QueueActivitySnapshot {
    QueueActivitySnapshot {
        local_active_items: local_queue_item_worker_ids(run_id),
        lease_active_items: state::active_web_queue_item_worker_lease_ids(run_id)
            .unwrap_or_else(|_| HashSet::new()),
    }
}

fn local_queue_item_worker_ids(run_id: &str) -> HashSet<String> {
    let prefix = format!("{run_id}:");
    WEB_QUEUE_ITEM_WORKERS
        .get()
        .and_then(|workers| workers.lock().ok())
        .map(|active| {
            active
                .iter()
                .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn queue_item_worker_active_in_snapshot(
    item_id: &str,
    activity: &QueueActivitySnapshot,
) -> bool {
    activity.local_active_items.contains(item_id) || activity.lease_active_items.contains(item_id)
}
