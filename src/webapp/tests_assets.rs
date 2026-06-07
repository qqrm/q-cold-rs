#[cfg(test)]
mod asset_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const EXPECTED_APP_JS_ASSETS: &[&str] = &[
        "src/webapp_assets/app/api.js",
        "src/webapp_assets/app/queue_status.js",
        "src/webapp_assets/app/telegram.js",
        "src/webapp_assets/app/init_parse.js",
        "src/webapp_assets/app/queue.js",
        "src/webapp_assets/app/queue_graph_diagnostics.js",
        "src/webapp_assets/app/terminal.js",
        "src/webapp_assets/app/events.js",
        "src/webapp_assets/app/queue_scroll.js",
        "src/webapp_assets/app/queue_transcript_lookup.js",
        "src/webapp_assets/app/events_bootstrap.js",
    ];

    #[test]
    fn app_js_bundle_matches_declared_asset_order() {
        assert_eq!(app_js_asset_paths(), EXPECTED_APP_JS_ASSETS);
        assert!(app_js_asset_paths().contains(&"src/webapp_assets/app/queue_scroll.js"));

        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut expected = String::new();
        for &asset in app_js_asset_paths() {
            expected.push_str(&std::fs::read_to_string(repo.join(asset)).unwrap());
        }

        assert_eq!(APP_JS, expected);
    }

    #[test]
    fn frontend_api_calls_use_shared_client() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for &asset in app_js_asset_paths() {
            let source = std::fs::read_to_string(repo.join(asset)).unwrap();
            if asset.ends_with("/api.js") {
                assert!(source.contains("fetch(path, requestOptions)"));
            } else {
                assert!(!source.contains("fetch("), "{asset} still calls fetch directly");
            }
        }

        assert!(APP_JS.find("const QcoldApi =").unwrap() < APP_JS.find("async function loadSnapshot").unwrap());
        assert!(APP_JS.contains("headers['x-qcold-write-token'] = token;"));
        assert!(APP_JS.contains("sessionStorage.setItem(writeTokenStorageKey, value);"));
        assert!(APP_JS.contains("Dashboard write token required; enter it in the header."));
        assert!(APP_JS.contains("function queueGraphDiagnosticMessages(payload)"));
        assert!(APP_JS.contains("function queueGraphResponseMessage(payload, fallback = 'request failed')"));
        assert!(INDEX_HTML.contains("id=\"write-token-input\""));
        assert!(!APP_JS.contains("QCOLD_WEBAPP_WRITE_TOKEN"));
    }

    #[test]
    fn frontend_queue_status_classification_uses_contract_helpers() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let contract = std::fs::read_to_string(repo.join("src/webapp_assets/app/queue_status.js"))
            .unwrap();
        assert!(contract.contains("const QcoldQueueStatus = (() => {"));
        assert!(contract.contains("const QueueRun = Object.freeze({"));
        assert!(contract.contains("const QueueItem = Object.freeze({"));
        assert!(contract.contains("const TaskRecord = Object.freeze({"));
        assert!(contract.contains("isQueueRunEditable"));
        assert!(contract.contains("isQueueItemTerminal"));
        assert!(contract.contains("isQueueItemPendingOrWaiting"));
        assert!(contract.contains("isTaskRecordClosedSuccess"));

        let forbidden = [
            ".status === 'closed:",
            ".status !== 'closed:",
            ".status?.startsWith('closed')",
            ".status.startsWith('closed')",
            "['running', 'waiting', 'starting', 'stopped']",
            "['starting', 'running', 'waiting']",
            "['starting', 'running']",
            "['success', 'failed', 'blocked']",
            "['pending', 'waiting']",
            "['stopped', 'paused']",
        ];
        for &asset in app_js_asset_paths() {
            if asset.ends_with("/queue_status.js") {
                continue;
            }
            let source = std::fs::read_to_string(repo.join(asset)).unwrap();
            for pattern in forbidden {
                assert!(!source.contains(pattern), "{asset} still contains {pattern}");
            }
        }
    }

    #[test]
    fn telegram_sdk_loading_is_optional_for_local_dashboard() {
        assert!(!INDEX_HTML.contains("telegram.org/js/telegram-web-app.js"));
        assert!(APP_JS.contains("const QcoldTelegram ="));
        assert!(APP_JS.contains("window.Telegram?.WebApp || null"));
        assert!(APP_JS.contains("tgWebApp(?:Data|Version|Platform|ThemeParams|StartParam)="));
        assert!(APP_JS.contains("script.async = true;"));
        assert!(APP_JS.contains("script.onerror = () => {"));
        assert!(APP_JS.contains("const tg = typeof QcoldTelegram === 'undefined' ? null : QcoldTelegram;"));
        assert!(APP_JS.contains("if (tg) tg.readyAndExpand();"));
        assert!(APP_JS.contains("if (tg) tg.applyTheme(value);"));
        assert!(APP_JS.contains("safeCall(app, 'showAlert', message);"));
    }

    #[test]
    fn web_terminal_slash_menu_uses_codex_command_prefixes() {
        assert!(APP_JS.contains("['model', 'choose what model and reasoning effort to use']"));
        assert!(APP_JS.contains(
            "['resume', 'resume a saved chat across Q-COLD worktrees', false, 'resume --all']"
        ));
        assert!(APP_JS.contains("['quit', 'exit Codex', true]"));
        assert!(APP_JS.contains("input.value = `/${terminalSlashCommandInsert(match)}`;"));
        assert!(APP_JS.contains("function terminalSlashCommandInsert(command)"));
        assert!(APP_JS.contains("function terminalSlashCommandMatches(query)"));
        assert!(APP_JS.contains("else if (name.startsWith(needle)) prefix.push(command);"));
        assert!(!APP_JS.contains("['/q'"));
        assert!(!APP_JS.contains("Help menu"));
    }

    #[test]
    fn graph_queue_active_run_keeps_wave_append_controls() {
        assert!(APP_JS.contains("addWaveButton.disabled = !queueGraphAppendable();"));
        assert!(APP_JS.contains("function queueBackendRunAppendable()"));
        assert!(APP_JS.contains("QcoldApi.appendQueue(runId, ["));
        assert!(APP_JS.contains("queueRunItemPayload({ ...item, dependsOn }, { prompt, dependsOn })"));
        assert!(APP_JS.contains(
            "queueWaves = normalizeQueueWaves(preservedWaves, queueItems, { pruneBackendEmpty: true });"
        ));
    }

    #[test]
    fn graph_queue_frontend_consumes_backend_diagnostics() {
        assert!(APP_JS.contains("QcoldApi.runQueue({"));
        assert!(APP_JS.contains("items: queueItems.map((item) => queueRunItemPayload(item, { selectedAgent }))"));
        assert!(APP_JS.contains("function queueGraphPayloadFields(item, dependsOn = item?.dependsOn || [])"));
        assert!(APP_JS.contains("fields.wave_id = item.waveId;"));
        assert!(APP_JS.contains("fields.wave_index = waveIndex;"));
        assert!(APP_JS.contains("function applyQueueGraphDiagnostics(payload, options = {})"));
        assert!(APP_JS.contains("applyQueueGraphCanonicalItems(graph);"));
        assert!(APP_JS.contains("QcoldApi.queueGraphResponseMessage(payload, 'failed to start backend queue')"));
        assert!(APP_JS.contains("QcoldApi.queueGraphResponseMessage(payload, 'failed to append queue item')"));
        assert!(APP_JS.contains("QcoldApi.queueGraphResponseMessage(payload, 'failed to update queue plan')"));
        assert!(APP_JS.contains("QcoldApi.queueGraphDiagnosticMessages(payload)"));
        assert!(!APP_JS.contains("function assignQueueWavesFromDependencies"));
    }

    #[test]
    fn backend_graph_queue_run_uses_execution_mode_not_status() {
        assert!(APP_JS.contains("executionMode: nextExecutionMode"));
        assert!(APP_JS.contains("queueRun.executionMode === QcoldQueueExecutionMode.Graph"));
        assert!(!APP_JS.contains("queueRun.status === QcoldQueueExecutionMode.Graph"));
    }

    #[test]
    fn queue_open_target_prefers_executor_before_transcript() {
        let target_start = APP_JS.find("function queueItemContextTarget").unwrap();
        let target = &APP_JS[target_start..];
        let terminal_index = target
            .find("const terminal = terminalForQueueItem(item, task);")
            .unwrap();
        let transcript_index = target.find("queueTaskTranscriptAvailable(item, task)").unwrap();

        assert!(terminal_index < transcript_index);
        assert!(APP_JS.contains("function queueTaskTranscriptAvailable(item, task)"));
        assert!(APP_JS.contains("return QcoldQueueStatus.isTaskRecordClosed(task);"));
        assert!(APP_JS.contains(
            "return { kind: 'task-modal', taskId: task.id, task, terminal };"
        ));
    }

    #[test]
    fn queue_terminal_lookup_accepts_server_agent_field_and_fallbacks() {
        assert!(APP_JS.contains("function queueItemAgentId(item, task = null)"));
        assert!(APP_JS.contains("function queueItemAgentIds(item, task = null)"));
        assert!(APP_JS.contains("[item?.agentId, item?.agent_id, task?.agent_id]"));
        assert!(APP_JS.contains("const agentIds = queueItemAgentIds(item, task);"));
        assert!(APP_JS.contains("if (agentIds.includes(terminal.agent_id)) return true;"));
        assert!(APP_JS.contains("if (taskId && terminal.scope === taskId) return true;"));
        assert!(APP_JS.contains(
            "return agentIds.some((agentId) => terminal.target === remoteNativeTerminalTarget(agentId));"
        ));
        assert!(APP_JS.contains("function remoteNativeTerminalTarget(agentId)"));
    }

    #[test]
    fn queue_terminal_lookup_is_after_terminal_chunk_tail() {
        let terminal_tail = APP_JS.find("function terminalKind(terminal)").unwrap();
        let lookup = APP_JS.find("function terminalForTaskId(taskId)").unwrap();
        let bootstrap = APP_JS
            .find("document.getElementById('close-transcript').addEventListener")
            .unwrap();

        assert!(terminal_tail < lookup);
        assert!(lookup < bootstrap);
    }

    #[test]
    fn task_chat_target_refreshes_open_modal_after_terminal_lookup() {
        let ensure_start = APP_JS.find("async function ensureTaskChatTarget(taskId)").unwrap();
        let ensure = &APP_JS[ensure_start..];
        let load_snapshot = ensure.find("await loadSnapshot();").unwrap();
        let terminal_lookup = ensure
            .find("const terminal = terminalForTarget(transcriptContext.terminalTarget);")
            .unwrap();
        let reopen = ensure
            .find("openTaskTranscript(taskId, { terminal });")
            .unwrap();

        assert!(load_snapshot < terminal_lookup);
        assert!(terminal_lookup < reopen);
    }

    #[test]
    fn graph_queue_active_run_prunes_empty_non_final_waves() {
        assert!(APP_JS.contains("function pruneEmptyBackendQueueWaves(waves, items)"));
        assert!(APP_JS.contains("function assignQueueWavesForDisplay(waves, items)"));
        assert!(APP_JS.contains("{ pruneBackendEmpty: true }"));
        assert!(APP_JS.contains("wavesWithItems.has(wave.id) || index === waves.length - 1"));
        assert!(APP_JS.contains(".filter((dependency) => byId.has(dependency))"));
    }

    #[test]
    fn graph_queue_refresh_preserves_wave_lane_scroll() {
        let render_start = APP_JS.find("function renderQueueGraph()").unwrap();
        let render = &APP_JS[render_start..];
        let capture = render
            .find("const scrollPositions = captureQueueWaveScrollPositions();")
            .unwrap();
        let replace = render.find("queueStatus.replaceChildren(board);").unwrap();
        let restore = render
            .find("restoreQueueWaveScrollPositions(scrollPositions);")
            .unwrap();

        assert!(capture < replace);
        assert!(replace < restore);
        assert!(APP_JS.contains("const queueWaveScrollPositions = new Map();"));
        assert!(APP_JS.contains("function captureQueueWaveScrollPositions()"));
        assert!(APP_JS.contains("function restoreQueueWaveScrollPositions(positions)"));
        assert!(APP_JS.contains("window.requestAnimationFrame(() => {"));
        assert!(APP_JS.contains("lane.scrollLeft = Math.min("));
    }

    #[test]
    fn queue_scroll_helpers_are_not_injected_inside_render_queue() {
        let render_start = APP_JS.find("function renderQueue()").unwrap();
        let sequence_render = APP_JS
            .find("queueStatus.replaceChildren(...queueItems.map")
            .unwrap();
        let graph_start = APP_JS.find("function renderQueueGraph()").unwrap();
        let final_fragment = APP_JS.find("async function loadAgentLimits(refresh)").unwrap();
        let capture_start = APP_JS
            .find("function captureQueueWaveScrollPositions()")
            .unwrap();
        let transcript_lookup = APP_JS.find("function terminalForTaskId(taskId)").unwrap();

        assert!(render_start < sequence_render);
        assert!(sequence_render < graph_start);
        assert!(graph_start < final_fragment);
        assert!(final_fragment < capture_start);
        assert!(capture_start < transcript_lookup);
    }

    #[test]
    fn queue_feedback_assets_are_embedded() {
        assert!(INDEX_HTML.contains("/assets/queue.css"));
        assert!(QUEUE_CSS.contains(".queue-toast-host"));
        assert!(APP_JS.contains("const removingQueueItems = new Map();"));
        assert!(APP_JS.contains("function queueItemRemovedOrRemoving(item)"));
        assert!(APP_JS.contains("let liveStateHoldUntil = 0;"));
    }

    #[test]
    fn dashboard_modals_have_a_dedicated_top_layer() {
        let strip = css_var_usize(APP_CSS, "--z-system-strip");
        let toast = css_var_usize(APP_CSS, "--z-dashboard-toast");
        let modal = css_var_usize(APP_CSS, "--z-dashboard-modal");

        assert!(strip < toast);
        assert!(toast < modal);
        assert!(QUEUE_CSS.contains("z-index: var(--z-dashboard-toast);"));
        assert!(APP_CSS.contains("z-index: var(--z-dashboard-modal);"));
        assert!(APP_CSS.contains("background: rgba(0, 0, 0, .74);"));
    }

    #[test]
    fn terminal_metadata_edit_uses_full_header_layout() {
        assert!(APP_CSS.contains(".terminal-head.editing-terminal-meta"));
        assert!(APP_JS.contains("classList.add('editing-terminal-meta')"));
        assert!(APP_JS.contains("classList.remove('editing-terminal-meta')"));
    }

    #[test]
    fn empty_local_queue_tab_does_not_hide_backend_active_run() {
        let selection_start = APP_JS.find("function selectedQueueTabId").unwrap();
        let selection = &APP_JS[selection_start..];
        assert!(APP_JS.contains("function queueTabHasLocalDraft(tabId)"));
        assert!(selection.contains("if (queueTabCreating && backendActiveTab) return backendActiveTab.id;"));
        assert!(selection.contains("&& currentTab.active"));
        assert!(selection.contains("if (!preserveDraft) return backendActiveRunTab.id;"));
        assert!(selection.contains("!queueTabHasLocalDraft(savedTab.id)"));
    }

    #[test]
    fn event_stream_errors_fall_back_to_state_polling() {
        assert!(APP_JS.contains("function startFallbackPolling()"));
        assert!(APP_JS.contains("eventSource.addEventListener('error', () => {"));
        assert!(APP_JS.contains("startFallbackPolling();"));
        assert!(APP_JS.contains("stopFallbackPolling();"));
    }

    #[test]
    fn dashboard_state_watcher_runs_without_manual_reload() {
        assert!(APP_JS.contains("const dashboardStateWatchPollMs = 2000;"));
        assert!(APP_JS.contains("let stateWatchTimer = null;"));
        assert!(APP_JS.contains("let snapshotRequestInFlight = false;"));
        assert!(APP_JS.contains("let lastSnapshotRenderKey = '';"));
        assert!(APP_JS.contains("function startStateWatcher()"));
        assert!(APP_JS.contains("window.setInterval(loadSnapshot, dashboardStateWatchPollMs);"));
        assert!(APP_JS.contains("if (snapshotRequestInFlight) return;"));
        assert!(APP_JS.contains("if (eventSource && eventSource.readyState !== EventSource.CLOSED) return;"));
        assert!(APP_JS.contains("startStateWatcher();\n    connectEvents();"));
        assert!(APP_JS.contains("document.addEventListener('visibilitychange', () => {"));
        assert!(APP_JS.contains("if (!document.hidden) {\n        loadSnapshot();"));
        assert!(APP_JS.contains("window.addEventListener('focus', loadSnapshot);"));
        assert!(APP_JS.contains("window.addEventListener('online', () => {"));
        assert!(!APP_JS.contains("if (document.hidden) {\n        if (eventSource) eventSource.close();"));
    }

    #[test]
    fn dashboard_state_watcher_skips_timestamp_only_rerenders() {
        assert!(APP_JS.contains("function snapshotRenderKey(snapshot)"));
        assert!(APP_JS.contains("const { generated_at_unix: _generatedAt, ...renderState } = nextState;"));
        assert!(APP_JS.contains("const renderKey = snapshotRenderKey(snapshot);"));
        assert!(APP_JS.contains("if (state && renderKey === lastSnapshotRenderKey) {"));
        assert!(APP_JS.contains("setLiveState('Live');\n        return;"));
        assert!(APP_JS.contains("lastSnapshotRenderKey = renderKey;"));
    }

    #[test]
    fn graph_queue_cards_show_backend_agent_activity() {
        assert!(APP_JS.contains("function queueGraphActivity(item, view)"));
        assert!(APP_JS.contains("function queueItemActivityLines(item, view = queueItemView(item))"));
        assert!(APP_JS.contains("function queueItemBackendActive(item)"));
        assert!(APP_JS.contains("runningAgent(agentId, item)"));
        assert!(APP_JS.contains("executionHost: item.execution_host || ''"));
        assert!(APP_JS.contains("terminal: ${terminalLine}"));
        assert!(QUEUE_CSS.contains(".queue-graph-activity-line"));
    }

    #[test]
    fn backend_graph_cards_use_backend_status_and_dependencies() {
        let view_start = APP_JS.find("function queueItemView(item)").unwrap();
        let view = &APP_JS[view_start..];
        assert!(
            view.find("if (queueBackendTerminalStatus(item))").unwrap()
                < view.find("if (activeAgentId)").unwrap()
        );
        assert!(APP_JS.contains("function queueBackendTerminalStatus(item)"));
        assert!(APP_JS.contains("function syncQueueGatesFromDependents(items = queueItems)"));
        assert!(APP_JS.contains("syncQueueGatesFromDependents(queueItems);"));
        assert!(APP_JS.contains("gatesNext: false"));
        assert!(APP_JS.contains(
            "if (queueHasBackendRun()) {\n        const dependents = queueDependentsForItem(item);"
        ));
        assert!(APP_JS.contains("function queueDependentsForItem(item)"));
        assert!(APP_JS.contains("No dependents"));
    }

    #[test]
    fn app_build_mismatch_warns_without_reloading() {
        assert!(APP_JS.contains("const appBuildId = String(window.__QCOLD_APP_BUILD_ID__ || '')"));
        assert!(APP_JS.contains("function snapshotBuildId(snapshot)"));
        assert!(APP_JS.contains("function noticeNewAppBuild(nextBuildId)"));
        assert!(APP_JS.contains("noticeNewAppBuild(snapshotBuildId(snapshot));"));
        assert!(APP_JS.contains("Dashboard assets changed; state updates remain live"));
        assert!(!APP_JS.contains("window.location.replace"));
        assert!(!APP_JS.contains("qcold_build"));
    }

    #[test]
    fn queue_tabs_assets_are_embedded() {
        assert!(INDEX_HTML.contains("id=\"queue-tabs\""));
        assert!(INDEX_HTML.contains("id=\"create-queue-tab\""));
        assert!(QUEUE_CSS.contains(".queue-tab"));
        assert!(QUEUE_CSS.contains(".queue-create"));
        assert!(QUEUE_CSS.contains("span:not(.queue-tab-close)"));
        assert!(QUEUE_CSS.contains("flex: 0 0 18px"));
        assert!(APP_JS.contains("function renderQueueTabs()"));
        assert!(APP_JS.contains("createQueueTabButton.addEventListener('click', createQueueTab)"));
        assert!(APP_JS.contains("const queueActiveTabStorageKey"));
        assert!(APP_JS.contains("function switchQueueTab(tabId)"));
        assert!(APP_JS.contains("localStorage.setItem(queueActiveTabStorageKey, activeQueueTabId)"));
        assert!(APP_JS.contains("queueTabSelectionUserTouched = false;"));
        assert!(APP_JS.contains("/api/queue/tab/create"));
        assert!(!APP_JS.contains("/api/queue/tab/switch"));
        assert!(APP_JS.contains("tab_id: activeQueueTabId"));
        assert!(APP_JS.contains("QcoldApi.clearQueue(runId)"));
    }

    fn css_var_usize(css: &str, name: &str) -> usize {
        let value = css
            .split_once(name)
            .and_then(|(_, rest)| rest.split_once(':'))
            .and_then(|(_, rest)| rest.split_once(';'))
            .map(|(value, _)| value.trim())
            .unwrap();
        value.parse().unwrap()
    }
}
