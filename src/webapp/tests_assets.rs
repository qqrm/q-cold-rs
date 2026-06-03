#[cfg(test)]
mod asset_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

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
        assert!(APP_JS.contains("items: [{ id: item.id, prompt, depends_on: dependsOn }]"));
        assert!(APP_JS.contains(
            "queueWaves = normalizeQueueWaves(preservedWaves, queueItems, { pruneBackendEmpty: true });"
        ));
    }

    #[test]
    fn queue_open_target_requires_executor_before_transcript() {
        let target_start = APP_JS.find("function queueItemContextTarget").unwrap();
        let target = &APP_JS[target_start..];
        let terminal_index = target
            .find("const terminal = terminalForQueueItem(item, task);")
            .unwrap();
        let transcript_index = target.find("queueTaskTranscriptAvailable(item, task)").unwrap();

        assert!(terminal_index < transcript_index);
        assert!(APP_JS.contains("function queueTaskTranscriptAvailable(item, task)"));
        assert!(APP_JS.contains("return Boolean(task.status?.startsWith('closed'));"));
        assert!(APP_JS.contains("if (task?.id) {\n        return null;"));
    }

    #[test]
    fn queue_terminal_lookup_accepts_server_agent_field() {
        assert!(APP_JS.contains("function queueItemAgentId(item, task = null)"));
        assert!(APP_JS.contains("return item?.agentId || item?.agent_id || task?.agent_id || '';"));
        assert!(APP_JS.contains("const agentId = queueItemAgentId(item, task);"));
        assert!(APP_JS.contains(
            "return (model?.terminals?.records || []).find((terminal) => terminal.agent_id === agentId) || null;"
        ));
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
    fn graph_queue_active_run_prunes_empty_non_final_waves() {
        assert!(APP_JS.contains("function pruneEmptyBackendQueueWaves(waves, items)"));
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
        assert!(APP_JS.contains("function renderQueueTabs()"));
        assert!(APP_JS.contains("createQueueTabButton.addEventListener('click', createQueueTab)"));
        assert!(APP_JS.contains("const queueActiveTabStorageKey"));
        assert!(APP_JS.contains("function switchQueueTab(tabId)"));
        assert!(APP_JS.contains("localStorage.setItem(queueActiveTabStorageKey, activeQueueTabId)"));
        assert!(APP_JS.contains("queueTabSelectionUserTouched = false;"));
        assert!(APP_JS.contains("/api/queue/tab/create"));
        assert!(!APP_JS.contains("/api/queue/tab/switch"));
        assert!(APP_JS.contains("tab_id: activeQueueTabId"));
        assert!(APP_JS.contains("body: JSON.stringify({ run_id: runId })"));
    }
}
