        if (shouldFollowTail) {
          scrollTerminalToTail(output);
        } else {
          output.scrollTop = Math.min(previousScrollTop, output.scrollHeight);
        }
        terminalOutputCache.set(terminal.target, nextOutput);
      }
      const input = node.querySelector('.terminal-input');
      if (input) input.placeholder = `send to ${terminalLabel(terminal)}`;
    }

    function isTerminalAtTail(output) {
      return output.scrollHeight - output.scrollTop - output.clientHeight <= 24;
    }

    function terminalShouldFollowTail(target, output) {
      if (!terminalTailLocks.has(target)) terminalTailLocks.set(target, true);
      return terminalTailLocks.get(target) || isTerminalAtTail(output);
    }

    function scrollTerminalToTail(output) {
      const scroll = () => {
        output.scrollTop = output.scrollHeight;
      };
      scroll();
      window.requestAnimationFrame(scroll);
    }

    function terminalKind(terminal) {
      if ((terminal.target || '').startsWith('zellij:')) return `zellij / ${terminal.pane}`;
      return `tmux / ${terminal.pane}`;
    }

    function terminalLabel(terminal) {
      return terminal.label || terminal.generated_label || (terminal.session || 'terminal').replace(/^qcold-/, '');
    }

    function terminalTitleControl(terminal) {
      const wrap = document.createElement('div');
      wrap.className = 'terminal-title';
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'terminal-title-button';
      button.title = 'Edit terminal name';
      button.textContent = terminalLabel(terminal);
      button.addEventListener('click', () => renderTerminalMetadataForm(wrap, terminal));
      wrap.appendChild(button);
      if (terminal.agent_id) {
        const agent = document.createElement('span');
        agent.className = 'terminal-scope';
        agent.textContent = shortAgentId(terminal.agent_id);
        wrap.appendChild(agent);
      }
      if (terminal.scope) {
        const scope = document.createElement('span');
        scope.className = 'terminal-scope';
        scope.textContent = terminal.scope;
        wrap.appendChild(scope);
      }
      return wrap;
    }

    function renderTerminalMetadataForm(wrap, terminal) {
      wrap.closest('.terminal-head')?.classList.add('editing-terminal-meta');
      const form = document.createElement('form');
      form.className = 'terminal-meta-form';
      const name = document.createElement('input');
      name.type = 'text';
      name.value = terminal.name || '';
      name.placeholder = terminal.generated_label || terminalLabel(terminal);
      name.maxLength = 80;
      const scope = document.createElement('input');
      scope.type = 'text';
      scope.value = terminal.scope || '';
      scope.placeholder = 'scope';
      scope.maxLength = 80;
      const save = document.createElement('button');
      save.type = 'submit';
      save.textContent = 'Save';
      const cancel = document.createElement('button');
      cancel.type = 'button';
      cancel.textContent = 'Cancel';
      cancel.addEventListener('click', () => {
        wrap.closest('.terminal-head')?.classList.remove('editing-terminal-meta');
        wrap.replaceWith(terminalTitleControl(terminal));
      });
      form.addEventListener('submit', async (event) => {
        event.preventDefault();
        wrap.closest('.terminal-head')?.classList.remove('editing-terminal-meta');
        await saveTerminalMetadata(terminal.target, name.value, scope.value);
      });
      form.append(name, scope, save, cancel);
      wrap.replaceChildren(form);
      name.focus();
    }

    function terminalComposer(terminal) {
      const compose = document.createElement('div');
      compose.className = 'terminal-compose';
      const input = document.createElement('textarea');
      input.className = 'terminal-input';
      input.placeholder = `send to ${terminalLabel(terminal)}`;
      input.value = terminalDrafts.get(terminal.target) || '';
      input.addEventListener('input', () => {
        terminalDrafts.set(terminal.target, input.value);
        updateTerminalSlashMenu(input);
      });
      input.addEventListener('keydown', (event) => {
        if (handleTerminalSlashMenuKey(input, event)) return;
        if (!input.value && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) {
          event.preventDefault();
          sendTerminalKey(terminal.target, terminalKeyName(event.key));
          return;
        }
        if (event.key === 'Enter' && !event.shiftKey) {
          event.preventDefault();
          sendTerminal(terminal.target, input);
        }
      });
      const button = document.createElement('button');
      button.type = 'button';
      button.textContent = 'Send';
      button.addEventListener('click', () => sendTerminal(terminal.target, input));
      compose.append(input, button);
      setupTerminalSlashMenu(input, compose, {
        onSelect: () => terminalDrafts.set(terminal.target, input.value),
      });
      return compose;
    }

    const terminalSlashCommands = [
      ['model', 'choose what model and reasoning effort to use'],
      ['fast', 'toggle Fast mode to enable fastest inference with increased plan usage'],
      ['ide', 'include current selection, open files, and other context from your IDE'],
      ['permissions', 'choose what Codex is allowed to do'],
      ['keymap', 'remap TUI shortcuts'],
      ['vim', 'toggle Vim mode for the composer'],
      ['setup-default-sandbox', 'set up elevated agent sandbox'],
      ['sandbox-add-read-dir', 'let sandbox read a directory: /sandbox-add-read-dir <absolute_path>'],
      ['experimental', 'toggle experimental features'],
      ['approve', 'approve one retry of a recent auto-review denial'],
      ['memories', 'configure memory use and generation'],
      ['skills', 'use skills to improve how Codex performs specific tasks'],
      ['hooks', 'view and manage lifecycle hooks'],
      ['review', 'review my current changes and find issues'],
      ['rename', 'rename the current thread'],
      ['new', 'start a new chat during a conversation'],
      ['resume', 'resume a saved chat across Q-COLD worktrees', false, 'resume --all'],
      ['fork', 'fork the current chat'],
      ['init', 'create an AGENTS.md file with instructions for Codex'],
      ['compact', 'summarize conversation to prevent hitting the context limit'],
      ['plan', 'switch to Plan mode'],
      ['goal', 'set or view the goal for a long-running task'],
      ['collab', 'change collaboration mode (experimental)'],
      ['agent', 'switch the active agent thread'],
      ['side', 'start a side conversation in an ephemeral fork'],
      ['copy', 'copy last response as markdown'],
      ['raw', 'toggle raw scrollback mode for copy-friendly terminal selection'],
      ['diff', 'show git diff (including untracked files)'],
      ['mention', 'mention a file'],
      ['status', 'show current session configuration and token usage'],
      ['title', 'configure which items appear in the terminal title'],
      ['statusline', 'configure which items appear in the status line'],
      ['theme', 'choose a syntax highlighting theme'],
      ['mcp', 'list configured MCP tools; use /mcp verbose for details'],
      ['plugins', 'browse plugins'],
      ['logout', 'log out of Codex'],
      ['quit', 'exit Codex', true],
      ['exit', 'exit Codex'],
      ['feedback', 'send logs to maintainers'],
      ['ps', 'list background terminals'],
      ['stop', 'stop all background terminals'],
      ['clear', 'clear the terminal and start a new chat'],
      ['personality', 'choose a communication style for Codex'],
      ['realtime', 'toggle realtime voice mode (experimental)'],
      ['settings', 'configure realtime microphone/speaker'],
      ['subagents', 'switch the active agent thread'],
    ];
    const terminalSlashMenus = new WeakMap();

    function setupTerminalSlashMenu(input, host, options = {}) {
      if (terminalSlashMenus.has(input)) return terminalSlashMenus.get(input);
      const menu = document.createElement('div');
      menu.className = 'terminal-slash-menu';
      menu.hidden = true;
      menu.setAttribute('role', 'listbox');
      host.classList.add('terminal-compose-shell');
      host.append(menu);
      const state = { menu, index: 0, matches: [], onSelect: options.onSelect || (() => {}) };
      terminalSlashMenus.set(input, state);
      input.addEventListener('input', () => updateTerminalSlashMenu(input));
      input.addEventListener('focus', () => updateTerminalSlashMenu(input));
      input.addEventListener('blur', () => {
        window.setTimeout(() => closeTerminalSlashMenu(input), 120);
      });
      return state;
    }

    function terminalSlashQuery(input) {
      const value = input.value || '';
      if (!value.startsWith('/') || value.includes('\n')) return null;
      if (/\s/.test(value)) return null;
      return value.slice(1).toLowerCase();
    }

    function updateTerminalSlashMenu(input) {
      const state = terminalSlashMenus.get(input);
      if (!state) return;
      const query = terminalSlashQuery(input);
      if (query === null) {
        closeTerminalSlashMenu(input);
        return;
      }
      state.matches = terminalSlashCommandMatches(query.trim());
      if (!state.matches.length) {
        closeTerminalSlashMenu(input);
        return;
      }
      state.index = Math.min(state.index, state.matches.length - 1);
      renderTerminalSlashMenu(input);
    }

    function renderTerminalSlashMenu(input) {
      const state = terminalSlashMenus.get(input);
      if (!state) return;
      state.menu.replaceChildren(...state.matches.map(([command, label], index) => {
        const item = document.createElement('button');
        item.type = 'button';
        item.className = index === state.index ? 'active' : '';
        item.setAttribute('role', 'option');
        item.setAttribute('aria-selected', index === state.index ? 'true' : 'false');
        item.addEventListener('mousedown', (event) => event.preventDefault());
        item.addEventListener('click', () => selectTerminalSlashCommand(input, index));
        const name = document.createElement('strong');
        name.textContent = `/${command}`;
        const hint = document.createElement('span');
        hint.textContent = label;
        item.append(name, hint);
        return item;
      }));
      state.menu.hidden = false;
    }

    function closeTerminalSlashMenu(input) {
      const state = terminalSlashMenus.get(input);
      if (!state) return;
      state.menu.hidden = true;
      state.menu.replaceChildren();
      state.matches = [];
      state.index = 0;
    }

    function selectTerminalSlashCommand(input, index) {
      const state = terminalSlashMenus.get(input);
      const match = state?.matches[index];
      if (!state || !match) return;
      input.value = `/${terminalSlashCommandInsert(match)}`;
      state.onSelect(input.value);
      closeTerminalSlashMenu(input);
      input.focus();
    }

    function terminalSlashCommandInsert(command) {
      return command[3] || command[0];
    }

    function handleTerminalSlashMenuKey(input, event) {
      const state = terminalSlashMenus.get(input);
      if (!state || state.menu.hidden) return false;
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        const delta = event.key === 'ArrowDown' ? 1 : -1;
        state.index = (state.index + delta + state.matches.length) % state.matches.length;
        renderTerminalSlashMenu(input);
        return true;
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        event.preventDefault();
        selectTerminalSlashCommand(input, state.index);
        return true;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        closeTerminalSlashMenu(input);
        return true;
      }
      return false;
    }

    function terminalSlashCommandMatches(query) {
      if (!query) return terminalSlashCommands.filter(([, , alias]) => !alias);
      const exact = [];
      const prefix = [];
      const needle = query.toLowerCase();
      terminalSlashCommands.forEach((command) => {
        const name = command[0].toLowerCase();
        if (name === needle) exact.push(command);
        else if (name.startsWith(needle)) prefix.push(command);
      });
      return exact.concat(prefix);
    }

    function messageNode(entry) {
      const node = document.createElement('article');
      node.className = `message ${entry.role || 'assistant'}`;
      const date = entry.timestamp ? new Date(entry.timestamp * 1000).toLocaleTimeString() : '';
      node.innerHTML = '<div class="message-head"><span></span><span></span></div><div class="message-text"></div>';
      node.children[0].children[0].textContent = `${entry.source || 'web'} / ${entry.role || 'assistant'}`;
      node.children[0].children[1].textContent = date;
      node.children[1].textContent = entry.text || '';
      return node;
    }

    function appendLocalMessage(role, text, source = 'web') {
      const label = String(text || '').trim() || 'No output';
      liveStateHoldUntil = Date.now() + 4200;
      setLiveState(label.length > 80 ? `${label.slice(0, 77)}...` : label, role === 'error' ? 'failed' : 'ready');
      appendQueueToast(role, label, source);
      if (role === 'error') console.error(`[${source}] ${label}`);
    }

    function appendQueueToast(role, text, source) {
      const host = queueToastHost();
      const toast = document.createElement('div');
      toast.className = `queue-toast ${role === 'error' ? 'failed' : 'ready'}`;
      const title = document.createElement('strong');
      title.textContent = source || 'web';
      const body = document.createElement('span');
      body.textContent = text;
      toast.append(title, body);
      host.appendChild(toast);
      while (host.children.length > 4) host.firstElementChild.remove();
      window.setTimeout(() => toast.classList.add('leaving'), 3600);
      window.setTimeout(() => toast.remove(), 4300);
    }

    function queueToastHost() {
      let host = document.getElementById('queue-toast-host');
      if (host) return host;
      host = document.createElement('div');
      host.id = 'queue-toast-host';
      host.className = 'queue-toast-host';
      host.setAttribute('aria-live', 'polite');
      document.body.appendChild(host);
      return host;
    }

    function renderSystemStrip() {
      const summary = model.status.summary;
      const openTasks = Number(summary.open_tasks || 0);
      const incomplete = Number(summary.incomplete_closeouts || 0);
      const dirty = Number(summary.primary_dirty || 0);
      document.getElementById('ready-pill').textContent = 'status';
      document.getElementById('ready-pill').className = 'badge';
      document.getElementById('repo-pill').textContent = `${state.repository.name} / ${state.repository.branch}`;
      const open = model.taskRecords.open || 0;
      const failed = model.taskRecords.failed || 0;
      const total = Math.max(model.taskRecords.count || 0, 1);
      const bar = document.getElementById('task-bar');
      bar.replaceChildren();
      const openSeg = document.createElement('div');
      openSeg.className = 'segment open';
      openSeg.style.flex = open || 0;
      const failedSeg = document.createElement('div');
      failedSeg.className = 'segment failed';
      failedSeg.style.flex = failed || 0;
      const idleSeg = document.createElement('div');
      idleSeg.className = 'segment idle';
      idleSeg.style.flex = Math.max(total - open - failed, 1);
      bar.append(openSeg, failedSeg, idleSeg);
      document.getElementById('strip-terminal').textContent = `${openTasks} worktree${openTasks === 1 ? '' : 's'}`;
      document.getElementById('strip-terminal').className = 'badge';
      document.getElementById('strip-repo').textContent = `${state.repository.name} / ${state.repository.branch}`;
      document.getElementById('strip-tasks').textContent = `${open} task records / `
        + `${openTasks} worktrees / ${failed} failed${dirty ? ` / ${dirty} dirty` : ''}`;
      document.getElementById('strip-closeouts').textContent = `${incomplete} closeout residue`;
      document.getElementById('strip-closeouts').className = incomplete ? 'strip-text bad' : 'strip-text';
      const hostRecords = model.hostAgents.records || [];
      const hostAgentCount = hostRecords.filter((agent) => agent.kind !== 'web-daemon').length;
      const daemonCount = hostRecords.length - hostAgentCount;
      document.getElementById('strip-agents').textContent = `${model.terminals.count} terminals / `
        + `${hostAgentCount} host${daemonCount ? ` / ${daemonCount} daemon` : ''}`;
    }

    function render() {
      if (!state) return;
      model = {
        status: parseStatus(state.status.text),
        agents: parseAgents(state.agents.text),
        taskRecords: state.task_records || { count: 0, open: 0, closed: 0, failed: 0, records: [] },
        queueTaskRecords: state.queue_task_records || { count: 0, open: 0, closed: 0, failed: 0, records: [] },
        queue: state.queue || { count: 0, running: false, run: null, records: [] },
        hostAgents: state.host_agents || { count: 0, records: [] },
        terminals: state.terminals || { count: 0, records: [] },
        availableAgents: state.available_agents || { count: 0, records: [] },
      };
      status.textContent = state.status.text;
      agents.textContent = state.agents.text;
      renderSystemStrip();
      renderTasks();
      renderAgents();
      renderTerminals();
      syncQueueFromSnapshot();
      renderQueue();
      if (document.getElementById('view-agents').classList.contains('active') && !agentLimits && !agentLimitsLoading) {
        window.setTimeout(() => loadAgentLimits(false), 0);
      }
      if (document.getElementById('view-queue').classList.contains('active') && !agentLimits && !agentLimitsLoading) {
        window.setTimeout(() => loadAgentLimits(false), 0);
      }
    }

    function setLiveState(label, tone = 'ready') {
      if (label === 'Live' && Date.now() < liveStateHoldUntil) return;
      liveState.textContent = label;
      liveState.className = `badge ${tone} live-indicator`;
    }

    function snapshotRenderKey(snapshot) {
      const nextState = snapshot?.state || null;
      if (!nextState) return '';
      const { generated_at_unix: _generatedAt, ...renderState } = nextState;
      return JSON.stringify(renderState);
    }

    function applySnapshot(snapshot) {
      noticeNewAppBuild(snapshotBuildId(snapshot));
      const renderKey = snapshotRenderKey(snapshot);
      if (state && renderKey === lastSnapshotRenderKey) {
        state = snapshot.state;
        setLiveState('Live');
        return;
      }
      state = snapshot.state;
      lastSnapshotRenderKey = renderKey;
      render();
      setLiveState('Live');
    }

    async function loadSnapshot() {
      if (snapshotRequestInFlight) return;
      snapshotRequestInFlight = true;
      try {
        const response = await fetch('/api/state', { cache: 'no-store' });
        if (!response.ok) throw new Error(`state refresh failed: ${response.status}`);
        applySnapshot({ state: await response.json() });
      } catch (err) {
        setLiveState('Offline', 'failed');
        if (!state) status.textContent = String(err);
      } finally {
        snapshotRequestInFlight = false;
      }
    }

    function startStateWatcher() {
      if (stateWatchTimer) return;
      loadSnapshot();
      stateWatchTimer = window.setInterval(loadSnapshot, dashboardStateWatchPollMs);
    }

    function startFallbackPolling() {
      startStateWatcher();
    }

    function stopFallbackPolling() {
      // State polling is now the baseline freshness watcher, not only an error fallback.
    }

    function connectEvents() {
      if (!window.EventSource) {
        startStateWatcher();
        return;
      }
      if (eventSource && eventSource.readyState !== EventSource.CLOSED) return;
      eventSource = new EventSource('/api/events');
      eventSource.addEventListener('snapshot', (event) => {
        stopFallbackPolling();
        applySnapshot(JSON.parse(event.data));
      });
      eventSource.addEventListener('error', () => {
        setLiveState('Offline', 'failed');
        startFallbackPolling();
      });
      eventSource.onopen = () => {
        stopFallbackPolling();
        setLiveState('Live');
      };
    }

    async function sendTerminal(target, input) {
      const trimmed = input.value.trimEnd();
      if (!trimmed.trim() || !target) return;
      input.value = '';
      terminalDrafts.delete(target);
      const payload = await postTerminalText(target, trimmed, {
        mode: terminalTextMode(trimmed),
        submit: true,
      });
      if (!payload.ok) appendLocalMessage('error', payload.output || 'failed to send terminal input');
      window.setTimeout(loadSnapshot, 250);
    }

    async function postTerminalText(target, text, options = {}) {
      try {
        const response = await fetch('/api/terminal/send', {
          method: 'POST',
          headers: {
            'content-type': 'application/json',
          },
          body: JSON.stringify({ target, text, ...options }),
        });
        const payload = await response.json();
        if (!response.ok && payload.ok !== false) payload.ok = false;
        return payload;
      } catch (err) {
        return { ok: false, output: String(err) };
      }
    }

    async function sendTerminalKey(target, key) {
      if (!target || !key) return;
      const payload = await postTerminalText(target, '', { mode: 'key', key });
      if (!payload.ok) appendLocalMessage('error', payload.output || 'failed to send terminal key');
      window.setTimeout(loadSnapshot, 100);
    }

    async function sendTerminalLiteral(target, text) {
      if (!target || !text) return;
      const payload = await postTerminalText(target, text, { mode: 'literal', submit: false });
      if (!payload.ok) appendLocalMessage('error', payload.output || 'failed to send terminal input');
      window.setTimeout(loadSnapshot, 100);
    }

    function handleTerminalKeyboard(event, target) {
      if (event.defaultPrevented || event.metaKey) return;
      const key = terminalKeyName(event.key);
      if (key) {
        event.preventDefault();
        sendTerminalKey(target, key);
        return;
      }
      if (!event.ctrlKey && !event.altKey && event.key.length === 1) {
        event.preventDefault();
        sendTerminalLiteral(target, event.key);
      }
    }

    function terminalKeyName(key) {
      const names = {
        ArrowUp: 'Up',
        ArrowDown: 'Down',
        ArrowLeft: 'Left',
        ArrowRight: 'Right',
        Enter: 'Enter',
        Backspace: 'Backspace',
        Delete: 'Delete',
        Escape: 'Escape',
        Tab: 'Tab',
        Home: 'Home',
        End: 'End',
        PageUp: 'PageUp',
        PageDown: 'PageDown',
      };
      return names[key] || '';
    }

    function terminalTextMode(text) {
      return text.trimStart().startsWith('/') && !text.includes('\n') ? 'literal' : 'paste';
    }

    async function saveTerminalMetadata(target, name, scope) {
      if (!target) return;
      try {
        const response = await fetch('/api/terminal/metadata', {
          method: 'POST',
          headers: {
            'content-type': 'application/json',
          },
          body: JSON.stringify({ target, name, scope }),
        });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) {
          appendLocalMessage('error', payload.output || 'Failed to save terminal metadata');
          return;
        }
        await loadSnapshot();
      } catch (err) {
        appendLocalMessage('error', String(err));
      }
    }

    function taskRecordForQueueItem(item) {
      const repo = queueItemRepository(item);
      const records = queueTaskRecords().filter((task) => task.id === `task/${item.slug}`);
      const agentId = queueItemAgentId(item);
      return records.find((task) => !repo?.root || task.repo_root === repo.root)
        || records.find((task) => agentId && task.agent_id === agentId)
        || null;
    }

    function queueItemBackendActive(item) {
      return Boolean(item?.runId && queueItemAgentId(item) && ['starting', 'running'].includes(item.status));
    }

    function runningAgent(agentId, item = null) {
      if (!agentId || !model) return true;
      const agents = model.agents?.records || [];
      if (agents.some((agent) => agent.id === agentId)) return true;
      const terminals = model.terminals?.records || [];
      if (terminals.some((terminal) => terminal.agent_id === agentId)) return true;
      return queueItemBackendActive(item) && queueItemAgentId(item) === agentId;
    }

    function terminalForAgentId(agentId) {
      if (!agentId) return null;
      return (model?.terminals?.records || []).find((terminal) => terminal.agent_id === agentId) || null;
    }

    function terminalPlainText(terminal) {
      return String(terminal?.output || '')
        .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '')
        .replace(/\x1b\][\s\S]*?(\x07|\x1b\\)/g, '');
    }

    function terminalCloseoutFailure(terminal) {
      const output = terminalPlainText(terminal);
      return [
        'Q-COLD closeout could not complete',
        'Could not complete canonical Q-COLD closeout',
        'missing task metadata',
        'repository target mismatch',
        'run this from a managed task worktree',
      ].some((needle) => output.includes(needle));
    }

    function terminalIdlePrompt(terminal) {
      const lines = terminalPlainText(terminal)
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean)
        .slice(-8);
      return lines.some((line) => line === '›' || line.startsWith('› '));
    }

    function terminalActivityLine(terminal) {
      const lines = terminalPlainText(terminal)
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean)
        .slice(-20)
        .reverse();
      const line = lines.find((value) => value !== '›' && !value.startsWith('› '));
      return compactQueueLine(line || '', 140);
    }

    function activeQueueAgentId(item, task = taskRecordForQueueItem(item)) {
      const agentId = queueItemAgentId(item, task);
      return agentId && runningAgent(agentId, item) ? agentId : '';
    }

    function queueRunIdFromSlug(slug) {
      const match = /^task-(.+)-\d+$/.exec(slug || '');
      return match?.[1] || '';
    }

    function existingQueueRunId() {
      for (const item of queueItems) {
        const runId = queueRunIdFromSlug(item.slug);
        if (runId) return runId;
      }
      return '';
    }

    async function runQueue() {
      if (queueRun.running || !queueItems.length) return;
      const selectedAgent = selectedQueueAgentRecord();
      if (!selectedAgent) return;
      appendLocalMessage('status', 'Starting queue run');
      if (queueGraphMode) syncQueueWaveDependencies();
      const now = Math.floor(Date.now() / 1000);
      const selectedRepo = selectedQueueRepository();
      queueRun = {
        running: true,
        stopped: false,
        stop: false,
        activeIndex: -1,
        runId: existingQueueRunId() || Date.now().toString(36),
        status: 'starting',
      };
      const usedSlugs = usedQueueSlugs(queueRun.runId);
      queueItems = queueItems.map((item, index) => {
        const slug = item.slug || nextQueueSlug(queueRun.runId, usedSlugs, index);
        const task = taskRecordForQueueItem(item);
        const repo = item.repoRoot ? queueItemRepository(item) : selectedRepo;
        const closedStatus = task?.status?.startsWith('closed') ? task.status : '';
        const success = closedStatus === 'closed:success' || item.status === 'success';
        const prompt = item.prompt.trim();
        const startsNow = prompt && !success && !closedStatus && queueItemStartsImmediately(item, index);
        const waiting = prompt && !success && !closedStatus && !startsNow;
        return {
          ...item,
          slug,
          agentId: queueItemAgentId(item, task),
          repoRoot: repo.root || '',
          repoName: repo.name || '',
          status: queueStartingStatus(success, closedStatus, startsNow, waiting),
          message: queueStartingMessage(success, closedStatus, startsNow, waiting, prompt),
          agentCommand: item.agentCommand || selectedAgent?.command || '',
          startedAt: item.startedAt || now,
          updatedAt: now,
        };
      });
      saveQueueStorage();
      renderQueue();
      try {
        const response = await fetch('/api/queue/run', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            run_id: queueRun.runId,
            tab_id: activeQueueTabId,
            execution_mode: queueGraphMode ? 'graph' : 'sequence',
            selected_agent_command: selectedAgent.command,
            selected_repo_root: selectedRepo.root || '',
            selected_repo_name: selectedRepo.name || '',
            items: queueItems.map((item) => ({
              id: item.id,
              prompt: item.prompt,
              slug: item.slug,
              depends_on: queueGraphMode ? (item.dependsOn || []) : [],
              repo_root: item.repoRoot,
              repo_name: item.repoName,
              agent_command: item.agentCommand || selectedAgent.command,
            })),
          }),
        });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) {
          queueRun = { running: false, stopped: false, stop: false, activeIndex: -1, runId: '', status: '' };
          queueItems[0].status = 'failed';
          queueItems[0].message = payload.output || 'failed to start backend queue';
          appendLocalMessage('error', queueItems[0].message);
          saveQueueStorage();
          renderQueue();
          return;
        }
      } catch (err) {
        queueRun = { running: false, stopped: false, stop: false, activeIndex: -1, runId: '', status: '' };
        queueItems[0].status = 'failed';
        queueItems[0].message = String(err);
        appendLocalMessage('error', queueItems[0].message);
        saveQueueStorage();
        renderQueue();
        return;
      }
      appendLocalMessage('status', 'Queue run accepted');
      await loadSnapshot();
    }

    async function createQueueTab() {
      if (queueTabCreating) return;
      queueTabCreating = true;
      renderQueue();
      if (!queueHasBackendRun()) saveQueueStorage();
      appendLocalMessage('status', 'Creating queue');
      try {
        const response = await fetch('/api/queue/tab/create', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ label: `Queue ${queueTabsModel.length + 1}` }),
        });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) {
          appendLocalMessage('error', payload.output || 'failed to create queue');
          return;
        }
        const tabId = String(payload.output || '').split('\t')[1] || '';
        if (tabId) {
          activeQueueTabId = tabId;
          queueTabSelectionUserTouched = false;
          localStorage.setItem(queueActiveTabStorageKey, activeQueueTabId);
        }
        await loadSnapshot();
      } catch (err) {
        appendLocalMessage('error', String(err));
      } finally {
        queueTabCreating = false;
        renderQueue();
      }
    }

    function switchQueueTab(tabId) {
      if (!tabId || tabId === activeQueueTabId) return;
      if (!queueHasBackendRun()) saveQueueStorage();
      activeQueueTabId = tabId;
      queueTabSelectionUserTouched = true;
      localStorage.setItem(queueActiveTabStorageKey, activeQueueTabId);
      syncQueueFromSnapshot();
      renderQueue();
    }

    async function deleteQueueTab(tabId) {
      const tab = queueTabsModel.find((candidate) => candidate.id === tabId);
      if (!tab || tab.isDefault || tab.running || queueTabDeletedOrDeleting(tabId)) return;
      rememberQueueTabDeletion(tabId);
      appendLocalMessage('status', 'Deleting queue');
      clearQueueDraft(tabId);
      const fallback = queueTabsModel.find((candidate) => candidate.id !== tabId);
      if (activeQueueTabId === tabId) {
        activeQueueTabId = fallback?.id || 'default';
        queueTabSelectionUserTouched = true;
        localStorage.setItem(queueActiveTabStorageKey, activeQueueTabId);
      }
      queueTabsModel = queueTabsModel.filter((candidate) => candidate.id !== tabId);
      syncQueueFromSnapshot();
      renderQueue();
      try {
        const response = await fetch('/api/queue/tab/delete', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ tab_id: tabId }),
        });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) {
          forgetQueueTabDeletion(tabId);
          appendLocalMessage('error', payload.output || 'failed to delete queue');
          await loadSnapshot();
          return;
        }
        await loadSnapshot();
      } catch (err) {
        forgetQueueTabDeletion(tabId);
        appendLocalMessage('error', String(err));
        await loadSnapshot();
      }
    }

    function queueItemStartsImmediately(item, index) {
      if (!queueGraphMode) return index === 0;
      return !(item.dependsOn || []).length;
    }

    function queueStartingMessage(success, closedStatus, startsNow, waiting, prompt) {
      if (success) return 'closed successfully';
      if (closedStatus) return closedStatus;
      if (startsNow) return 'starting backend queue';
      if (waiting) return 'waiting for prior wave';
      return prompt ? '' : 'empty prompt';
    }

    function queueStartingStatus(success, closedStatus, startsNow, waiting) {
      if (success) return 'success';
      if (closedStatus) return 'failed';
      if (startsNow) return 'starting';
      return waiting ? 'waiting' : 'failed';
    }

    async function stopQueue() {
      if (queueRun.stopped) {
        await continueQueue();
        return;
      }
      const runId = queueRun.runId || queueItems.find((item) => item.runId)?.runId || '';
      if (!runId) return;
      queueRun.stop = true;
      appendLocalMessage('status', 'Stop requested');
      try {
        const response = await fetch('/api/queue/stop', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ run_id: runId }),
        });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) {
          appendLocalMessage('error', payload.output || 'failed to stop queue');
          return;
        }
        await loadSnapshot();
      } catch (err) {
        appendLocalMessage('error', String(err));
      }
      renderQueue();
    }

    async function continueQueue() {
      const runId = queueRun.runId || queueItems.find((item) => item.runId)?.runId || '';
      if (!runId) return;
      const previousQueueRun = { ...queueRun };
      const previousQueueItems = queueItems;
      queueRun = {
        ...queueRun,
        running: true,
        stopped: false,
        stop: false,
        runId,
        status: 'starting',
      };
      queueItems = queueItems.map((item) => {
        if (!['stopped', 'paused'].includes(item.status)) return item;
        return { ...item, status: 'starting', message: 'continuing queue' };
      });
      appendLocalMessage('status', 'Continuing queue');
      saveQueueStorage();
      renderQueue();
      try {
        const response = await fetch('/api/queue/continue', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ run_id: runId }),
        });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) {
          appendLocalMessage('error', payload.output || 'failed to continue queue');
          queueRun = previousQueueRun;
          queueItems = previousQueueItems;
          saveQueueStorage();
          renderQueue();
          return;
        }
        await loadSnapshot();
      } catch (err) {
        appendLocalMessage('error', String(err));
        queueRun = previousQueueRun;
        queueItems = previousQueueItems;
        saveQueueStorage();
      }
      renderQueue();
    }

    function preferredView() {
      const fromHash = window.location.hash.replace(/^#/, '');
      if (fromHash === 'start') return 'queue';
      if (viewNames.has(fromHash)) return fromHash;
      const stored = localStorage.getItem('qcold-view') || '';
      if (stored === 'start') return 'queue';
      return viewNames.has(stored) ? stored : 'queue';
    }

    function setActiveView(view, persist = true) {
      const next = viewNames.has(view) ? view : 'queue';
      viewButtons.forEach((button) => button.classList.toggle('active', button.dataset.view === next));
      document.querySelectorAll('.view').forEach((item) => item.classList.remove('active'));
      document.getElementById(`view-${next}`).classList.add('active');
      if (persist) {
        localStorage.setItem('qcold-view', next);
        if (window.location.hash !== `#${next}`) {
          history.replaceState(null, '', `#${next}`);
        }
      }
      if ((next === 'agents' || next === 'queue') && model) loadAgentLimits(false);
    }

    async function loadAgentLimits(refresh) {
      if (!model) return;
      if (agentLimitsLoading) return;
      agentLimitsLoading = true;
      renderAgents();
      renderQueue();
      try {
        const response = await fetch(`/api/agent-limits${refresh ? '?refresh=true' : ''}`, { cache: 'no-store' });
        agentLimits = await response.json();
      } catch (err) {
        agentLimits = {
          cached: false,
          count: 0,
          records: [],
        };
        appendLocalMessage('error', `Failed to load agent limits: ${err}`);
      } finally {
        agentLimitsLoading = false;
        renderAgents();
        renderQueue();
      }
    }
