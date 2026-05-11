#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{
        cargo_subcommand_args, codex_account_from_agent_command,
        codex_task_telemetry_for_worktree_in_roots,
        find_codex_session_summary_in_root, is_queue_agent_track, parse_codex_session_summary,
        parse_rfc3339_unix, polish_task_text, prompt_from_agent_command, render_token_efficiency,
        render_token_usage, slug_from_title, task_flow_metadata_equivalent, unix_now,
    };
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn jsonl(value: serde_json::Value) -> String {
        format!("{value}\n")
    }

    fn session_meta_event(cwd: &str) -> serde_json::Value {
        serde_json::json!({
            "timestamp": "1970-01-01T00:00:01.000Z",
            "type": "session_meta",
            "payload": {
                "id": "019df1ab-7579-7e41-ad71-701b63175455",
                "timestamp": "1970-01-01T00:00:01Z",
                "cwd": cwd,
            },
        })
    }

    fn token_count_event(
        second: u8,
        input: u64,
        cached: u64,
        output: u64,
        reasoning: u64,
        total: u64,
    ) -> serde_json::Value {
        serde_json::json!({
            "timestamp": format!("1970-01-01T00:00:{second:02}.000Z"),
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": input,
                        "cached_input_tokens": cached,
                        "output_tokens": output,
                        "reasoning_output_tokens": reasoning,
                        "total_tokens": total,
                    },
                    "model_context_window": 258400,
                },
            },
        })
    }

    #[test]
    fn cargo_plugin_invocation_strips_subcommand_name() {
        assert_eq!(
            cargo_subcommand_args(os_args(&["cargo-qcold", "qcold", "status"])),
            os_args(&["qcold", "status"])
        );
    }

    #[test]
    fn direct_invocation_is_preserved() {
        assert_eq!(
            cargo_subcommand_args(os_args(&["qcold", "status"])),
            os_args(&["qcold", "status"])
        );
    }

    #[test]
    fn task_text_is_polished_for_storage() {
        assert_eq!(
            polish_task_text("Сделай, блядь, CRUD для задач"),
            "Сделай, CRUD для задач"
        );
    }

    #[test]
    fn c2_command_prompt_is_detected() {
        assert_eq!(
            prompt_from_agent_command("/home/qqrm/.local/bin/c2 \"Добавь CRUD для задач\"")
                .as_deref(),
            Some("Добавь CRUD для задач")
        );
        assert_eq!(
            prompt_from_agent_command("/home/qqrm/.local/bin/cc1 \"Проверь сабмодули\"")
                .as_deref(),
            Some("Проверь сабмодули")
        );
    }

    #[test]
    fn queue_agent_tracks_are_internal_launchers() {
        assert!(is_queue_agent_track("queue-moz964nn"));
        assert!(is_queue_agent_track("queue-run-123"));
        assert!(!is_queue_agent_track("queue"));
        assert!(!is_queue_agent_track("queue:manual"));
        assert!(!is_queue_agent_track("c1"));
    }

    #[test]
    fn codex_account_is_detected_from_cc2_wrapper() {
        assert_eq!(
            codex_account_from_agent_command("/home/qqrm/.local/bin/cc1").as_deref(),
            Some("1")
        );
        assert_eq!(
            codex_account_from_agent_command("/home/qqrm/.local/bin/cc2").as_deref(),
            Some("2")
        );
        assert_eq!(
            codex_account_from_agent_command("/usr/bin/codex3 exec inspect").as_deref(),
            Some("3")
        );
    }

    #[test]
    fn codex_session_summary_imports_prompt_and_token_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(
            "rollout-2026-05-04T09-27-19-019df1ab-7579-7e41-ad71-701b63175455.jsonl",
        );
        fs::write(
            &path,
            [
                jsonl(serde_json::json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "019df1ab-7579-7e41-ad71-701b63175455",
                        "timestamp": "2026-05-04T09:27:19Z",
                        "cwd": "/workspace/repo",
                    },
                })),
                jsonl(serde_json::json!({
                    "type": "event_msg",
                    "payload": {
                        "type": "user_message",
                        "message": "Сделай CRUD для задач",
                        "images": [],
                    },
                })),
                jsonl(serde_json::json!({
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 100,
                                "cached_input_tokens": 40,
                                "output_tokens": 9,
                                "reasoning_output_tokens": 3,
                                "total_tokens": 109,
                            },
                            "last_token_usage": {
                                "input_tokens": 100,
                            },
                            "model_context_window": 258400,
                        },
                        "rate_limits": {
                            "plan_type": "pro",
                        },
                    },
                })),
                jsonl(serde_json::json!({
                    "type": "event_msg",
                    "payload": {
                        "type": "task_complete",
                    },
                })),
            ]
            .concat(),
        )
        .unwrap();

        let summary = parse_codex_session_summary(&path).unwrap().unwrap();
        assert_eq!(summary.prompt, "Сделай CRUD для задач");
        assert_eq!(
            summary.thread_id.as_deref(),
            Some("019df1ab-7579-7e41-ad71-701b63175455")
        );
        assert!(summary.task_complete);
        assert_eq!(
            summary.started_at,
            Some(parse_rfc3339_unix("2026-05-04T09:27:19Z").unwrap())
        );
        assert_eq!(summary.cwd.as_deref(), Some("/workspace/repo"));
        let usage = summary.token_usage.unwrap();
        assert_eq!(usage["non_cached_input_tokens"], 60);
        assert_eq!(usage["displayed_total_tokens"], 69);
    }

    #[test]
    fn codex_token_usage_for_worktree_sums_last_usage_window() {
        let temp = tempfile::tempdir().unwrap();
        let session_dir = temp.path().join("sessions/2026/05/06");
        let worktree = temp.path().join("WT/qcold/anchor-token-task");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join(
                "rollout-2026-05-06T00-00-00-019df1ab-7579-7e41-ad71-701b63175455.jsonl",
            ),
            [
                jsonl(serde_json::json!({
                    "timestamp": "1970-01-01T00:00:01.000Z",
                    "type": "session_meta",
                    "payload": {
                        "id": "019df1ab-7579-7e41-ad71-701b63175455",
                        "timestamp": "1970-01-01T00:00:01Z",
                        "cwd": worktree.display().to_string(),
                    },
                })),
                jsonl(serde_json::json!({
                    "timestamp": "1970-01-01T00:00:02.000Z",
                    "type": "response_item",
                    "payload": {
                        "type": "function_call",
                        "arguments": serde_json::json!({
                            "workdir": worktree.display().to_string(),
                        })
                        .to_string(),
                    },
                })),
                jsonl(token_count_event(3, 10, 4, 2, 1, 12)),
                jsonl(token_count_event(4, 7, 5, 3, 2, 10)),
            ]
            .concat(),
        )
        .unwrap();

        let telemetry = codex_task_telemetry_for_worktree_in_roots(
            &worktree,
            None,
            0,
            u64::MAX,
            &[temp.path().join("sessions")],
            Some(0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(telemetry.session_paths.len(), 1);
        assert!(telemetry.session_paths[0].ends_with(
            "rollout-2026-05-06T00-00-00-019df1ab-7579-7e41-ad71-701b63175455.jsonl"
        ));
        let usage = telemetry.usage;
        assert_eq!(usage.model_calls, 2);
        assert_eq!(usage.input_tokens, 17);
        assert_eq!(usage.cached_input_tokens, 9);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.reasoning_output_tokens, 3);
        assert_eq!(usage.total_tokens, 22);
    }

    #[test]
    fn codex_task_telemetry_matches_task_slug_and_records_tool_output_stats() {
        let temp = tempfile::tempdir().unwrap();
        let session_dir = temp.path().join("sessions/2026/05/06");
        let worktree = temp.path().join("WT/qcold/anchor-token-task");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join(
                "rollout-2026-05-06T00-00-00-019df1ab-7579-7e41-ad71-701b63175455.jsonl",
            ),
            [
                jsonl(session_meta_event("/workspace/repo")),
                jsonl(serde_json::json!({
                    "timestamp": "1970-01-01T00:00:02.000Z",
                    "type": "response_item",
                    "payload": {
                        "type": "function_call",
                        "name": "exec_command",
                        "arguments": serde_json::json!({
                            "cmd": "rg -n token src",
                            "workdir": worktree.display().to_string(),
                        })
                        .to_string(),
                        "call_id": "call_big",
                    },
                })),
                jsonl(serde_json::json!({
                    "timestamp": "1970-01-01T00:00:03.000Z",
                    "type": "response_item",
                    "payload": {
                        "type": "function_call_output",
                        "call_id": "call_big",
                        "output": "Chunk ID: abc\nOriginal token count: 6001\nOutput:\ntask/token-task\n",
                    },
                })),
                jsonl(token_count_event(4, 10, 4, 2, 1, 12)),
            ]
            .concat(),
        )
        .unwrap();

        let telemetry = codex_task_telemetry_for_worktree_in_roots(
            &worktree,
            Some("token-task"),
            0,
            u64::MAX,
            &[temp.path().join("sessions")],
            Some(0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(telemetry.session_count, 1);
        assert_eq!(
            telemetry.session_ids,
            ["019df1ab-7579-7e41-ad71-701b63175455"]
        );
        assert_eq!(telemetry.session_paths.len(), 1);
        assert_eq!(telemetry.matched_by_worktree, 1);
        assert_eq!(telemetry.matched_by_task, 1);
        assert_eq!(telemetry.usage.model_calls, 1);
        assert_eq!(telemetry.tool_outputs.calls, 1);
        assert_eq!(telemetry.tool_outputs.original_tokens, 6001);
        assert_eq!(telemetry.tool_outputs.large_calls, 1);
        assert!(
            telemetry.tool_outputs.samples[0]
                .command
                .contains("rg -n token src")
        );
    }

    #[test]
    fn codex_task_telemetry_ignores_task_slug_without_structured_worktree_match() {
        let temp = tempfile::tempdir().unwrap();
        let session_dir = temp.path().join("sessions/2026/05/06");
        let worktree = temp.path().join("WT/vitastor/392-task-mp0by95n-04");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join(
                "rollout-2026-05-06T00-00-00-019df1ab-7579-7e41-ad71-701b63175455.jsonl",
            ),
            [
                jsonl(session_meta_event("/home/qqrm/repos/github/qcold")),
                jsonl(serde_json::json!({
                    "timestamp": "1970-01-01T00:00:02.000Z",
                    "type": "response_item",
                    "payload": {
                        "type": "function_call",
                        "name": "exec_command",
                        "arguments": serde_json::json!({
                            "cmd": "pgrep -af task-mp0by95n",
                            "workdir": "/home/qqrm/repos/github/qcold",
                        })
                        .to_string(),
                        "call_id": "call_noise",
                    },
                })),
                jsonl(serde_json::json!({
                    "timestamp": "1970-01-01T00:00:03.000Z",
                    "type": "response_item",
                    "payload": {
                        "type": "function_call_output",
                        "call_id": "call_noise",
                        "output": format!(
                            "Original token count: 6001\nOutput:\n{} task/task-mp0by95n-04\n",
                            worktree.display()
                        ),
                    },
                })),
                jsonl(token_count_event(4, 10, 4, 2, 1, 12)),
            ]
            .concat(),
        )
        .unwrap();

        let telemetry = codex_task_telemetry_for_worktree_in_roots(
            &worktree,
            Some("task-mp0by95n-04"),
            0,
            u64::MAX,
            &[temp.path().join("sessions")],
            Some(0),
        )
        .unwrap();
        assert_eq!(telemetry, None);
    }

    #[test]
    fn codex_task_telemetry_retention_cutoff_skips_old_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let session_dir = temp.path().join("sessions/2026/05/06");
        let worktree = temp.path().join("WT/qcold/anchor-token-task");
        fs::create_dir_all(&worktree).unwrap();
        fs::create_dir_all(&session_dir).unwrap();
        let session_path = session_dir.join(
            "rollout-2026-05-06T00-00-00-019df1ab-7579-7e41-ad71-701b63175455.jsonl",
        );
        fs::write(
            &session_path,
            [
                jsonl(serde_json::json!({
                    "timestamp": "1970-01-01T00:00:02.000Z",
                    "type": "session_meta",
                    "payload": {
                        "id": "019df1ab-7579-7e41-ad71-701b63175455",
                        "timestamp": "1970-01-01T00:00:02Z",
                        "cwd": worktree.display().to_string(),
                    },
                })),
                jsonl(serde_json::json!({
                    "timestamp": "1970-01-01T00:00:03.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": 10,
                                "total_tokens": 10,
                            },
                        },
                    },
                })),
            ]
            .concat(),
        )
        .unwrap();

        let telemetry = codex_task_telemetry_for_worktree_in_roots(
            &worktree,
            Some("token-task"),
            0,
            u64::MAX,
            &[temp.path().join("sessions")],
            Some(unix_now().saturating_add(1)),
        )
        .unwrap();
        assert!(telemetry.is_none());
    }

    #[test]
    fn token_usage_renderer_prints_task_record_fields() {
        let usage = serde_json::json!({
            "input_tokens": 17,
            "cached_input_tokens": 9,
            "non_cached_input_tokens": 8,
            "output_tokens": 5,
            "reasoning_output_tokens": 3,
            "total_tokens": 22,
            "displayed_total_tokens": 13,
            "model_calls": 2,
            "model_context_window": 258400,
            "source": "codex-session-window",
        });
        assert_eq!(
            render_token_usage(&usage).as_deref(),
            Some(concat!(
                "token-usage\tinput=17\tcached_input=9\tnon_cached_input=8\toutput=5",
                "\treasoning=3\ttotal=22\tdisplayed=13\tmodel_calls=2",
                "\tcontext=258400\tsource=codex-session-window",
            ))
        );
    }

    #[test]
    fn token_efficiency_renderer_prints_compact_fields() {
        let efficiency = serde_json::json!({
            "source": "codex-session-window",
            "session_count": 2,
            "matched_by_worktree": 1,
            "matched_by_task": 1,
            "tool_output_original_tokens": 7000,
            "large_tool_output_calls": 1,
            "large_tool_output_original_tokens": 6001,
            "retention_hours": 48,
        });
        assert_eq!(
            render_token_efficiency(&efficiency).as_deref(),
            Some(concat!(
                "token-efficiency\tsessions=2\tmatched_worktree=1\tmatched_task=1",
                "\ttool_output_tokens=7000\tlarge_tool_outputs=1",
                "\tlarge_tool_output_tokens=6001\tretention_hours=48",
                "\tsource=codex-session-window",
            ))
        );
    }

    #[test]
    fn task_flow_metadata_equivalence_ignores_capture_timestamp_only() {
        let left = serde_json::json!({
            "kind": "managed-task-flow",
            "token_efficiency": {
                "source": "codex-session-window",
                "captured_at": 10,
                "tool_output_original_tokens": 7000,
            },
        });
        let right = serde_json::json!({
            "kind": "managed-task-flow",
            "token_efficiency": {
                "source": "codex-session-window",
                "captured_at": 20,
                "tool_output_original_tokens": 7000,
            },
        });
        assert!(task_flow_metadata_equivalent(&left, &right));

        let changed = serde_json::json!({
            "kind": "managed-task-flow",
            "token_efficiency": {
                "source": "codex-session-window",
                "captured_at": 20,
                "tool_output_original_tokens": 7001,
            },
        });
        assert!(!task_flow_metadata_equivalent(&left, &changed));
    }

    #[test]
    fn codex_session_matcher_uses_session_start_and_claims() {
        let dir = tempfile::tempdir().unwrap();
        let claimed = dir.path().join(
            "rollout-1970-01-01T00-00-10-019df1ab-7579-7e41-ad71-701b63175455.jsonl",
        );
        let selected = dir.path().join(
            "rollout-1970-01-01T00-00-11-019df1ab-7579-7e41-ad71-701b63175456.jsonl",
        );
        let stale = dir.path().join(
            "rollout-1970-01-01T00-30-00-019df1ab-7579-7e41-ad71-701b63175457.jsonl",
        );
        write_session(&claimed, "1970-01-01T00:00:10Z", "/workspace/repo", "claimed");
        write_session(&selected, "1970-01-01T00:00:11Z", "/workspace/repo", "selected");
        write_session(&stale, "1970-01-01T00:30:00Z", "/workspace/repo", "stale");

        let claimed_paths = HashSet::from([claimed.display().to_string()]);
        let summary = find_codex_session_summary_in_root(
            dir.path(),
            10,
            &claimed_paths,
            Some(Path::new("/workspace/repo")),
        )
        .unwrap()
        .unwrap();
        assert_eq!(summary.path, selected);
        assert_eq!(summary.prompt, "selected");
    }

    #[test]
    fn rfc3339_timestamp_parser_handles_codex_session_meta() {
        assert_eq!(
            parse_rfc3339_unix("1970-01-02T00:00:01.123Z"),
            Some(86_401)
        );
    }

    #[test]
    fn task_slug_is_ascii_and_stable() {
        assert_eq!(slug_from_title("Fix task CRUD flow"), "fix-task-crud-flow");
        assert_eq!(slug_from_title("Задача"), "task");
    }

    fn write_session(path: &Path, timestamp: &str, cwd: &str, prompt: &str) {
        fs::write(
            path,
            format!(
                "{}\n{}\n",
                format_args!(
                    "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"019df1ab-7579-7e41-ad71-701b63175455\",\
                     \"timestamp\":\"{timestamp}\",\"cwd\":\"{cwd}\"}}}}"
                ),
                format_args!(
                    "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\
                     \"message\":\"{prompt}\",\"images\":[]}}}}"
                )
            ),
        )
        .unwrap();
    }
}
