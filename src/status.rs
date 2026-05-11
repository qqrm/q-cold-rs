use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::{repository, state};

pub fn run() -> Result<u8> {
    if let Err(err) = crate::sync_codex_task_records() {
        eprintln!("warning: failed to refresh Codex task token telemetry: {err:#}");
    }
    print!("{}", snapshot()?);
    if let Some(summary) = task_record_token_snapshot()? {
        print!("{summary}");
    }
    Ok(0)
}

pub fn snapshot() -> Result<String> {
    snapshot_for(&repository::active_root()?)
}

pub fn snapshot_for(primary_root: &Path) -> Result<String> {
    let primary_root = primary_root.to_path_buf();
    let managed_root = managed_root_for(&primary_root);
    let dirty_paths = git_dirty_paths(&primary_root)?;
    let tasks = managed_tasks(&managed_root)?;
    let incomplete = tasks
        .iter()
        .filter(|task| task.status == "failed-closeout")
        .count();
    let terminal_ready = dirty_paths.is_empty() && tasks.is_empty();
    let mut lines = Vec::new();
    lines.push(format!(
        "qcold-status\tterminal_ready={}\topen_tasks={}\tinterrupted_tasks={}\t\
         incomplete_closeouts={}\tprimary_dirty={}\toverlaps={}",
        if terminal_ready { "yes" } else { "no" },
        tasks.len(),
        0,
        incomplete,
        dirty_paths.len(),
        0,
    ));
    lines.push(format!(
        "primary\t{}\tmanaged_root={}\tbranch_context=primary",
        primary_root.display(),
        managed_root.display(),
    ));

    for task in tasks {
        lines.push(format!(
            "task\t{}\t{}\t{}\tstate=attached",
            task.name,
            task.status,
            task.worktree.display(),
        ));
    }

    for path in dirty_paths {
        lines.push(format!("primary-dirty-file\t{path}"));
    }

    Ok(format!("{}\n", lines.join("\n")))
}

fn managed_tasks(managed_root: &Path) -> Result<Vec<ManagedTask>> {
    if !managed_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut tasks = Vec::new();
    for entry in std::fs::read_dir(managed_root)
        .with_context(|| format!("failed to read {}", managed_root.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let worktree = entry.path();
        let env = worktree.join(".task/task.env");
        if !env.is_file() {
            continue;
        }
        let data = parse_env_file(&env)?;
        let raw_name = data
            .get("TASK_NAME")
            .cloned()
            .or_else(|| {
                data.get("TASK_BRANCH")
                    .and_then(|branch| branch.strip_prefix("task/").map(ToOwned::to_owned))
            })
            .or_else(|| {
                worktree
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(task_name_from_worktree_component)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| "unknown".to_string());
        let status = data
            .get("STATUS")
            .filter(|value| !value.is_empty())
            .cloned()
            .unwrap_or_else(|| "open".to_string());
        if !task_blocks_terminal(&status) {
            continue;
        }
        tasks.push(ManagedTask {
            name: raw_name,
            status,
            worktree,
        });
    }
    tasks.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(tasks)
}

fn task_blocks_terminal(status: &str) -> bool {
    status.is_empty() || status == "open" || status == "paused" || status == "failed-closeout"
}

struct ManagedTask {
    name: String,
    status: String,
    worktree: PathBuf,
}

fn parse_env_file(path: &Path) -> Result<std::collections::BTreeMap<String, String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let mut entries = std::collections::BTreeMap::new();
    let mut pending = String::new();
    for line in content.lines() {
        if pending.is_empty() && line.trim().is_empty() {
            continue;
        }
        if pending.is_empty() {
            pending.push_str(line);
        } else {
            pending.push('\n');
            pending.push_str(line);
        }
        if let Some((key, value)) = parse_env_entry(&pending) {
            entries.insert(key, value);
            pending.clear();
        }
    }
    if !pending.is_empty() {
        bail!("invalid task env line in {}: {pending}", path.display());
    }
    Ok(entries)
}

fn parse_env_entry(line: &str) -> Option<(String, String)> {
    let (key, raw) = line.split_once('=')?;
    if !(raw.starts_with('\'') && raw.ends_with('\'')) {
        return Some((key.to_string(), raw.to_string()));
    }
    Some((key.to_string(), raw[1..raw.len() - 1].replace("'\\''", "'")))
}

fn task_name_from_worktree_component(value: &str) -> &str {
    let mut parts = value.splitn(4, '-');
    let (Some(_color), Some(_fruit), Some(number), Some(task_name)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return value;
    };
    if number.len() == 3 && number.chars().all(|ch| ch.is_ascii_digit()) {
        task_name
    } else {
        value
    }
}

fn managed_root_for(primary_root: &Path) -> PathBuf {
    primary_root.parent().map_or_else(
        || primary_root.join("WT"),
        |parent| {
            parent
                .join("WT")
                .join(primary_root.file_name().unwrap_or_default())
        },
    )
}

fn git_dirty_paths(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain"])
        .output()
        .context("failed to inspect git status")?;
    if !output.status.success() {
        bail!("git status failed with status {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.get(3..))
        .map(str::to_string)
        .collect())
}

fn task_record_token_snapshot() -> Result<Option<String>> {
    let records = state::load_task_records(None, 1000)?;
    let mut count = 0u64;
    let mut displayed = 0u64;
    let mut total = 0u64;
    let mut output = 0u64;
    let mut reasoning = 0u64;
    let mut model_calls = 0u64;
    let mut efficiency_count = 0u64;
    let mut sessions = 0u64;
    let mut tool_output_tokens = 0u64;
    let mut large_tool_outputs = 0u64;
    let mut large_tool_output_tokens = 0u64;
    for record in records {
        let Some(metadata) = record
            .metadata_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        else {
            continue;
        };
        let Some(usage) = metadata.get("token_usage") else {
            continue;
        };
        count += 1;
        displayed += usage
            .get("displayed_total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        total += usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        output += usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        reasoning += usage
            .get("reasoning_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        model_calls += usage
            .get("model_calls")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if let Some(efficiency) = metadata.get("token_efficiency") {
            efficiency_count += 1;
            sessions += efficiency
                .get("session_count")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            tool_output_tokens += efficiency
                .get("tool_output_original_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            large_tool_outputs += efficiency
                .get("large_tool_output_calls")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            large_tool_output_tokens += efficiency
                .get("large_tool_output_original_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
        }
    }
    if count == 0 {
        return Ok(None);
    }
    let mut lines = vec![format!(
        "task-record-tokens\trecords={count}\tdisplayed={displayed}\ttotal={total}\
         \toutput={output}\treasoning={reasoning}\tmodel_calls={model_calls}"
    )];
    if efficiency_count > 0 {
        lines.push(format!(
            "task-record-efficiency\trecords={efficiency_count}\tsessions={sessions}\
             \ttool_output_tokens={tool_output_tokens}\tlarge_tool_outputs={large_tool_outputs}\
             \tlarge_tool_output_tokens={large_tool_output_tokens}"
        ));
    }
    Ok(Some(format!("{}\n", lines.join("\n"))))
}

pub fn telegram_snapshot() -> Result<String> {
    Ok(format_snapshot_for_telegram(&snapshot()?))
}

fn format_snapshot_for_telegram(snapshot: &str) -> String {
    let mut summary = StatusSummary::default();
    let mut primary = None;
    let mut tasks = Vec::new();
    let mut dirty_paths = Vec::new();
    let mut overlaps = Vec::new();

    for line in snapshot.lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["qcold-status", values @ ..] => summary = StatusSummary::parse(values),
            ["primary", root, rest @ ..] => {
                let managed_root = rest
                    .iter()
                    .find_map(|field| field.strip_prefix("managed_root="))
                    .unwrap_or("unknown");
                primary = Some(PrimarySummary {
                    root: (*root).to_string(),
                    managed_root: managed_root.to_string(),
                });
            }
            ["task", name, status, path, state] => tasks.push(TaskSummary {
                name: (*name).to_string(),
                status: humanize_status(status),
                path: (*path).to_string(),
                state: state
                    .strip_prefix("state=")
                    .unwrap_or(state)
                    .replace('-', " "),
            }),
            ["primary-dirty-file", path] => dirty_paths.push((*path).to_string()),
            ["open-task-dirty-overlap", task, path, _worktree] => {
                overlaps.push(format!("{task}: {path}"));
            }
            _ => {}
        }
    }

    let mut lines = Vec::new();
    lines.push("Q-COLD status".to_string());
    lines.push(format!(
        "Terminal ready: {}",
        if summary.terminal_ready { "yes" } else { "no" }
    ));

    if let Some(primary) = primary {
        lines.push(format!("Repository: {}", short_path(&primary.root)));
        lines.push(format!(
            "Managed worktrees: {}",
            short_path(&primary.managed_root)
        ));
    }

    lines.push(format!(
        "Tasks: {} attached, {} interrupted, {} incomplete closeout",
        summary.open_tasks, summary.interrupted_tasks, summary.incomplete_closeouts
    ));
    lines.push(format!(
        "Primary checkout: {} dirty file{}, {} overlap{}",
        summary.primary_dirty,
        plural(summary.primary_dirty),
        summary.overlaps,
        plural(summary.overlaps)
    ));

    if !tasks.is_empty() {
        lines.push(String::new());
        lines.push("Tasks".to_string());
        for (index, task) in tasks.iter().enumerate() {
            lines.push(format!("{}. {}", index + 1, task.name));
            lines.push(format!("   status: {}", task.status));
            lines.push(format!("   state: {}", task.state));
            lines.push(format!("   worktree: {}", short_path(&task.path)));
        }
    }

    if !dirty_paths.is_empty() {
        lines.push(String::new());
        lines.push("Primary dirty files".to_string());
        for path in dirty_paths {
            lines.push(format!("- {path}"));
        }
    }

    if !overlaps.is_empty() {
        lines.push(String::new());
        lines.push("Dirty overlaps".to_string());
        for overlap in overlaps {
            lines.push(format!("- {overlap}"));
        }
    }

    if !summary.terminal_ready {
        lines.push(String::new());
        lines.push("Next step".to_string());
        if summary.incomplete_closeouts > 0 {
            lines.push("Resolve failed closeouts or clear stale task state.".to_string());
        } else if summary.open_tasks > 0 {
            lines.push("Finish or close the attached tasks.".to_string());
        } else if summary.primary_dirty > 0 || summary.overlaps > 0 {
            lines.push("Clean the primary checkout drift.".to_string());
        }
    }

    lines.join("\n")
}

#[derive(Default)]
struct StatusSummary {
    terminal_ready: bool,
    open_tasks: usize,
    interrupted_tasks: usize,
    incomplete_closeouts: usize,
    primary_dirty: usize,
    overlaps: usize,
}

impl StatusSummary {
    fn parse(fields: &[&str]) -> Self {
        let mut summary = Self::default();
        for field in fields {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            match key {
                "terminal_ready" => summary.terminal_ready = value == "yes",
                "open_tasks" => summary.open_tasks = value.parse().unwrap_or_default(),
                "interrupted_tasks" => {
                    summary.interrupted_tasks = value.parse().unwrap_or_default();
                }
                "incomplete_closeouts" => {
                    summary.incomplete_closeouts = value.parse().unwrap_or_default();
                }
                "primary_dirty" => summary.primary_dirty = value.parse().unwrap_or_default(),
                "overlaps" => summary.overlaps = value.parse().unwrap_or_default(),
                _ => {}
            }
        }
        summary
    }
}

struct PrimarySummary {
    root: String,
    managed_root: String,
}

struct TaskSummary {
    name: String,
    status: String,
    path: String,
    state: String,
}

fn humanize_status(status: &str) -> String {
    status.replace('-', " ")
}

fn short_path(path: &str) -> String {
    let Some(index) = path.find("/repos/github/") else {
        return path.to_string();
    };
    path[index + 1..].to_string()
}

fn plural(value: usize) -> &'static str {
    if value == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_status_is_human_readable() {
        let raw = concat!(
            "qcold-status\tterminal_ready=no\topen_tasks=3\tinterrupted_tasks=0\t",
            "incomplete_closeouts=2\tprimary_dirty=0\toverlaps=0\n",
            "primary\t/workspace/repos/github/repository\t",
            "managed_root=/workspace/repos/github/WT/repository\tbranch_context=primary\n",
            "task\treview-batch-4\tfailed-closeout\t",
            "/workspace/repos/github/WT/repository/purple-kiwi-998-review-batch-4\tstate=attached\n",
            "task\tprimary-read-lifecycle-live-surface-broadening\topen\t",
            "/workspace/repos/github/WT/repository/red-persimmon-893-primary-read-lifecycle-live-surface-broadening\t",
            "state=attached\n",
        );
        let formatted = format_snapshot_for_telegram(raw);
        assert!(formatted.contains("Q-COLD status"));
        assert!(formatted.contains("Terminal ready: no"));
        assert!(formatted.contains("Tasks: 3 attached, 0 interrupted, 2 incomplete closeout"));
        assert!(formatted.contains("1. review-batch-4"));
        assert!(formatted.contains("status: failed closeout"));
        assert!(!formatted.contains("qcold-status\t"));
    }

    #[test]
    fn managed_tasks_ignore_terminal_closed_statuses() {
        let temp = tempfile::tempdir().unwrap();
        let managed = temp.path().join("WT").join("repo");
        write_task_env(&managed.join("001-open"), "open", "open");
        write_task_env(&managed.join("002-blocked"), "blocked", "closed:blocked");
        write_task_env(&managed.join("003-failed-closeout"), "tail", "failed-closeout");

        let tasks = managed_tasks(&managed).unwrap();

        assert_eq!(
            tasks.iter().map(|task| task.name.as_str()).collect::<Vec<_>>(),
            vec!["open", "tail"]
        );
    }

    fn write_task_env(worktree: &Path, name: &str, status: &str) {
        let task_dir = worktree.join(".task");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(
            task_dir.join("task.env"),
            format!(
                "TASK_NAME={name}\nSTATUS={status}\nTASK_WORKTREE={}\n",
                worktree.display()
            ),
        )
        .unwrap();
    }
}
