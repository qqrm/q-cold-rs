fn clean_queue_run_id(value: &str) -> String {
    sanitize_daemon_id(value)
}

fn clean_queue_slug(
    value: &str,
    run_id: &str,
    index: usize,
    used_slugs: &mut HashSet<String>,
) -> String {
    let mut slug = sanitize_daemon_id(value);
    if slug.is_empty() {
        slug = queue_slug(run_id, index);
    }
    while !used_slugs.insert(slug.clone()) {
        slug = queue_slug(run_id, used_slugs.len());
    }
    slug
}

fn queue_track(run_id: &str) -> String {
    format!("queue-{}", sanitize_daemon_id(run_id))
}

fn queue_agent_id(item: &state::QueueItemRow) -> String {
    let slug = sanitize_daemon_id(&item.slug);
    if slug.len() <= 36 {
        format!("qa-{slug}")
    } else {
        let prefix = slug.chars().take(24).collect::<String>();
        format!("qa-{prefix}-{}", stable_short_hash(&item.id))
    }
}

fn queue_slug(run_id: &str, index: usize) -> String {
    format!("task-{}-{:02}", sanitize_daemon_id(run_id), index + 1)
}
