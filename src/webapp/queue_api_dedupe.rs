fn ensure_queue_run_conflict_free(
    run: &state::QueueRunRow,
    items: &[state::QueueItemRow],
) -> Result<()> {
    let requested = items
        .iter()
        .map(|item| item.slug.clone())
        .collect::<HashSet<_>>();
    let requested_keys = items
        .iter()
        .map(|item| queue_task_conflict_key(&item.slug))
        .collect::<HashSet<_>>();
    if requested.is_empty() {
        return Ok(());
    }
    let Some(conflict) = state::load_web_queue_items()?.into_iter().find(|other| {
        (requested.contains(&other.slug)
            || requested_keys.contains(&queue_task_conflict_key(&other.slug)))
            && other.run_id != run.id
            && queue_item_blocks_new_run(&other.status)
    }) else {
        return Ok(());
    };
    bail!(
        "queue task task/{} already exists in run {} with status {}; \
         use queue append/continue or clear/delete the stale run before creating another queue",
        conflict.slug,
        conflict.run_id,
        conflict.status
    )
}

fn queue_item_blocks_new_run(status: &state::QueueItemStatus) -> bool {
    !status.is_success()
}

fn queue_task_conflict_key(slug: &str) -> String {
    let mut parts = slug.split('-').collect::<Vec<_>>();
    if parts.last().is_some_and(|part| queue_slug_date_token(part)) {
        parts.pop();
    }

    let mut normalized = Vec::with_capacity(parts.len());
    let mut index = 0;
    while index < parts.len() {
        if parts[index] == "after" && parts.get(index + 1) == Some(&"repair") {
            index += 2;
            continue;
        }
        if queue_slug_retry_port(parts[index]) {
            index += 1;
            continue;
        }
        normalized.push(parts[index]);
        index += 1;
    }

    if normalized.is_empty() {
        slug.to_string()
    } else {
        normalized.join("-")
    }
}

fn queue_slug_date_token(token: &str) -> bool {
    token.len() == 8 && token.starts_with("20") && token.chars().all(|ch| ch.is_ascii_digit())
}

fn queue_slug_retry_port(token: &str) -> bool {
    token
        .strip_prefix('p')
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
}
