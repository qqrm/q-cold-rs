fn cleanup_queue_item_artifacts(
    item: &state::QueueItemRow,
    task_id: Option<&str>,
    agent_id: Option<&str>,
) -> Result<()> {
    let default_task_id = format!("task/{}", item.slug);
    let task_id = task_id
        .filter(|id| !id.trim().is_empty())
        .map_or(default_task_id, str::to_string);
    let task = state::get_task_record(&task_id)?
        .filter(|task| queue_task_record_matches_item(item, task));
    let agent_id = agent_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .or_else(|| item.agent_id.clone())
        .or_else(|| task.as_ref().and_then(|task| task.agent_id.clone()));
    if queue_item_remote_native(item) {
        if task.is_some() {
            state::delete_task_record(&task_id)?;
        }
        if let Some(agent_id) = agent_id {
            let _ = cleanup_queue_executor(item, &agent_id);
        }
        return Ok(());
    }
    cleanup_existing_task_agent_artifacts(&task_id, task.as_ref(), agent_id)
}

fn cleanup_task_agent_artifacts(task_id: Option<&str>, agent_id: Option<&str>) -> Result<()> {
    let task_id = task_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string);
    let task = task_id
        .as_deref()
        .map(state::get_task_record)
        .transpose()?
        .flatten();
    let agent_id = agent_id
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .or_else(|| task.as_ref().and_then(|task| task.agent_id.clone()));
    if let Some(task_id) = task_id {
        cleanup_existing_task_agent_artifacts(&task_id, task.as_ref(), agent_id)?;
    } else if let Some(agent_id) = agent_id {
        let _ = agents::terminate_agent(&agent_id);
    }
    Ok(())
}
