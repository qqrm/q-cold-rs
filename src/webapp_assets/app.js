const tg = window.Telegram && window.Telegram.WebApp;
    if (tg) { tg.ready(); tg.expand(); }

    let state = null;
    let model = null;
    const status = document.getElementById('status');
    const agents = document.getElementById('agents');
    const tasks = document.getElementById('tasks');
    const agentList = document.getElementById('agent-list');
    const availableAgentList = document.getElementById('available-agent-list');
    const agentLimitState = document.getElementById('agent-limit-state');
    const hostAgentList = document.getElementById('host-agent-list');
    const terminalList = document.getElementById('terminal-list');
    const queueInput = document.getElementById('queue-input');
    const queueRepoSelect = document.getElementById('queue-repo');
    const queueAgentSelect = document.getElementById('queue-agent');
    const queueAgentState = document.getElementById('queue-agent-state');
    const queueState = document.getElementById('queue-state');
    const queueStatus = document.getElementById('queue-status');
    const chatLog = document.getElementById('chat-log');
    const chatInput = document.getElementById('chat-input');
    const transcriptModal = document.getElementById('transcript-modal');
    const transcriptTitle = document.getElementById('transcript-title');
    const transcriptSubtitle = document.getElementById('transcript-subtitle');
    const transcriptLog = document.getElementById('transcript-log');
    const transcriptCompose = document.getElementById('transcript-compose');
    const transcriptInput = document.getElementById('transcript-input');
    const transcriptSend = document.getElementById('send-transcript');
    const themeButtons = Array.from(document.querySelectorAll('[data-theme-choice]'));
    const liveState = document.getElementById('live-state');
    let fallbackTimer = null;
    let eventSource = null;
    const terminalDrafts = new Map();
    const terminalOutputCache = new Map();
    const viewButtons = Array.from(document.querySelectorAll('.nav button'));
    const viewNames = new Set(viewButtons.map((button) => button.dataset.view));
    const queueStorageKey = 'qcold-task-queue-v4';
    const queueAgentStorageKey = 'qcold-task-queue-agent-v1';
    const queueRepoStorageKey = 'qcold-task-queue-repo-v1';
    let selectedQueueAgent = localStorage.getItem(queueAgentStorageKey) || '';
    let selectedQueueRepoRoot = localStorage.getItem(queueRepoStorageKey) || '';
    const queueSaved = loadQueueStorage();
    let queueItems = (queueSaved.items || [])
      .map((item) => ({ ...defaultQueueItem(), ...item }));
    let queueRun = { running: false, stop: false, activeIndex: -1, runId: '' };
    let transcriptContext = { taskId: '', terminalTarget: '', chatAvailable: false };
    let agentLimits = null;
    let agentLimitsLoading = false;

    function applyTheme(choice) {
      const value = choice || localStorage.getItem('qcold-theme') || 'auto';
      document.documentElement.dataset.theme = value === 'auto' ? '' : value;
      themeButtons.forEach((button) => {
        button.classList.toggle('active', button.dataset.themeChoice === value);
      });
      localStorage.setItem('qcold-theme', value);
      if (tg) {
        tg.setHeaderColor(value === 'dark' ? '#101114' : 'secondary_bg_color');
        tg.setBackgroundColor(value === 'dark' ? '#101114' : 'bg_color');
      }
    }

    function parseKeyValues(parts) {
      return Object.fromEntries(parts.map((part) => {
        const i = part.indexOf('=');
        return i === -1 ? [part, ''] : [part.slice(0, i), part.slice(i + 1)];
      }));
    }

    function parseStatus(text) {
      const lines = text.trim().split('\n').filter(Boolean);
      const result = { summary: {}, primary: null, tasks: [] };
      for (const line of lines) {
        const parts = line.split('\t');
        if (parts[0] === 'qcold-status') result.summary = parseKeyValues(parts.slice(1));
        if (parts[0] === 'primary') result.primary = {
          root: parts[1],
          meta: parseKeyValues(parts.slice(2)),
        };
        if (parts[0] === 'task') result.tasks.push({
          slug: parts[1],
          status: parts[2],
          path: parts[3],
          meta: parseKeyValues(parts.slice(4)),
        });
      }
      return result;
    }

    function parseAgents(text) {
      const lines = text.trim().split('\n').filter(Boolean);
      const summary = lines[0] ? parseKeyValues(lines[0].split('\t').slice(1)) : { count: '0' };
      const allRecords = lines.slice(1).map((line) => {
        const parts = line.split('\t');
        return { id: parts[1] || 'agent', meta: parseKeyValues(parts.slice(2)) };
      });
      const records = allRecords.filter((agent) => agent.meta.state === 'running');
      return { count: Number(summary.count || 0), runningCount: records.length, records };
    }

    function badge(status) {
      const span = document.createElement('span');
      const tone = status.includes('failed') ? 'failed' : status === 'open' ? 'open' : status.includes('blocked') ? 'warn' : 'ready';
      span.className = `badge ${tone}`;
      span.textContent = status;
      return span;
    }

    function shortAgentId(agentId) {
      if (!agentId) return '';
      const parts = String(agentId).split('-').filter(Boolean);
      if (parts.length >= 2) return `${parts[0]}-${parts[parts.length - 1].slice(-4)}`;
      return String(agentId).slice(0, 12);
    }

    function agentLabelForId(agentId, task = null) {
      if (!agentId && task?.agent_label) return task.agent_label;
      const terminal = (model?.terminals?.records || []).find((record) => record.agent_id === agentId);
      if (terminal) return terminalLabel(terminal);
      const agent = (model?.agents?.records || []).find((record) => record.id === agentId);
      if (agent?.meta?.name) return agent.meta.name;
      if (task?.agent_label) return task.agent_label;
      return shortAgentId(agentId);
    }

    function agentBadgeText(agentId, task = null) {
      const label = agentLabelForId(agentId, task);
      if (!label) return '';
      const shortId = shortAgentId(agentId);
      return shortId && shortId !== label ? `agent ${label} / ${shortId}` : `agent ${label}`;
    }

    function loadQueueStorage() {
      try {
        return JSON.parse(localStorage.getItem(queueStorageKey) || '{}');
      } catch (_err) {
        return {};
      }
    }

    function saveQueueStorage() {
      localStorage.setItem(queueStorageKey, JSON.stringify({
        items: queueItems.map((item) => ({
          id: item.id,
          prompt: item.prompt,
          slug: item.slug,
          agentId: item.agentId,
          agentCommand: item.agentCommand,
          repoRoot: item.repoRoot,
          repoName: item.repoName,
          status: item.status,
          message: item.message,
          startedAt: item.startedAt,
          updatedAt: item.updatedAt,
        })),
      }));
    }

    function defaultQueueItem() {
      return {
        id: `queue-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
        prompt: '',
        slug: '',
        agentId: '',
        agentCommand: '',
        repoRoot: '',
        repoName: '',
        status: 'pending',
        message: '',
        startedAt: 0,
        updatedAt: 0,
      };
    }

    function queueSlug(runId, index) {
      return `task-${runId}-${String(index + 1).padStart(2, '0')}`;
    }

    function queueTaskRecords() {
      return [
        ...(state?.task_records?.records || []),
        ...(state?.queue_task_records?.records || []),
      ];
    }

    function usedQueueSlugs(runId) {
      const used = new Set(queueItems.map((item) => item.slug).filter(Boolean));
      const prefix = `task/task-${runId}-`;
      for (const task of queueTaskRecords()) {
        if (task.id?.startsWith(prefix)) {
          used.add(task.id.slice('task/'.length));
        }
      }
      return used;
    }

    function nextQueueSlug(runId, used, preferredIndex) {
      let index = Math.max(preferredIndex, 0);
      let slug = queueSlug(runId, index);
      while (used.has(slug)) {
        index += 1;
        slug = queueSlug(runId, index);
      }
      used.add(slug);
      return slug;
    }

    function queueRepositories() {
      const repos = state?.repositories?.length ? state.repositories : [state?.repository].filter(Boolean);
      const byRoot = new Map();
      for (const repo of repos) {
        if (repo?.root && !byRoot.has(repo.root)) byRoot.set(repo.root, repo);
      }
      if (state?.repository?.root && !byRoot.has(state.repository.root)) {
        byRoot.set(state.repository.root, state.repository);
      }
      return Array.from(byRoot.values());
    }

    function selectedQueueRepository() {
      const repos = queueRepositories();
      return repos.find((repo) => repo.root === selectedQueueRepoRoot)
        || repos.find((repo) => repo.active)
        || repos[0]
        || state?.repository
        || { root: '', name: 'repository' };
    }

    function queueItemRepository(item) {
      const repos = queueRepositories();
      return repos.find((repo) => repo.root === item?.repoRoot)
        || (item?.repoRoot ? { root: item.repoRoot, name: item.repoName || item.repoRoot } : null)
        || selectedQueueRepository();
    }

    function renderQueueRepoSelector() {
      const repos = queueRepositories();
      const current = selectedQueueRepoRoot || queueRepoSelect.value;
      const nextSelected = repos.some((repo) => repo.root === current)
        ? current
        : (repos.find((repo) => repo.active)?.root || repos[0]?.root || '');
      selectedQueueRepoRoot = nextSelected;
      queueRepoSelect.replaceChildren(...repos.map((repo) => {
        const option = document.createElement('option');
        option.value = repo.root;
        option.textContent = `${repo.name}${repo.active ? ' *' : ''}`;
        option.title = repo.root;
        return option;
      }));
      if (!repos.length) {
        const option = document.createElement('option');
        option.value = '';
        option.textContent = 'No repositories found';
        queueRepoSelect.appendChild(option);
      }
      queueRepoSelect.value = selectedQueueRepoRoot;
      queueRepoSelect.disabled = queueRun.running || !repos.length;
      if (selectedQueueRepoRoot) localStorage.setItem(queueRepoStorageKey, selectedQueueRepoRoot);
    }

    function queueAgentRecords() {
      const records = model?.availableAgents?.records || [];
      const numberedAccounts = records.some((agent) => /^\d+$/.test(agent.account || ''));
      const byAccount = new Map();
      for (const agent of records) {
        if (agent.command?.startsWith('cc')) continue;
        if (agent.account === 'default' && numberedAccounts) continue;
        const key = agent.account || agent.command;
        const previous = byAccount.get(key);
        if (!previous || queueAgentPreference(agent) < queueAgentPreference(previous)) {
          byAccount.set(key, agent);
        }
      }
      return Array.from(byAccount.values());
    }

    function queueAgentPreference(agent) {
      if (/^c\d+$/.test(agent.command || '')) return 0;
      if (/^codex\d+$/.test(agent.command || '')) return 1;
      if (agent.command === 'codex') return 2;
      return 3;
    }

    function selectedQueueAgentRecord() {
      const records = queueAgentRecords();
      return records.find((agent) => agent.command === selectedQueueAgent) || records[0] || null;
    }

    function queueAgentLimit(agent) {
      if (!agent) return null;
      return (agentLimits?.records || []).find((record) => (
        record.command === agent.command || record.account === agent.account
      )) || null;
    }

    function queueAgentStatusLabel(limit) {
      if (agentLimitsLoading) return 'checking';
      if (!limit) return 'not checked';
      if (limit.state === 'unauthenticated') return 'not logged in';
      return limit.state || 'unknown';
    }

    function renderQueueAgentSelector() {
      const records = queueAgentRecords();
      const current = selectedQueueAgent || queueAgentSelect.value;
      const nextSelected = records.some((agent) => agent.command === current)
        ? current
        : (records[0]?.command || '');
      selectedQueueAgent = nextSelected;
      queueAgentSelect.replaceChildren(...records.map((agent) => {
        const limit = queueAgentLimit(agent);
        const option = document.createElement('option');
        option.value = agent.command;
        option.textContent = `${agent.command} - ${agent.label} / ${queueAgentStatusLabel(limit)}`;
        option.title = [agent.path || agent.command, limit?.summary || ''].filter(Boolean).join('\n');
        return option;
      }));
      if (!records.length) {
        const option = document.createElement('option');
        option.value = '';
        option.textContent = 'No agent commands found';
        queueAgentSelect.appendChild(option);
      }
      queueAgentSelect.value = selectedQueueAgent;
      queueAgentSelect.disabled = queueRun.running || !records.length;
      const detected = model?.availableAgents?.records?.length || 0;
      const okCount = records.filter((agent) => queueAgentLimit(agent)?.state === 'ok').length;
      queueAgentState.textContent = agentLimitsLoading
        ? 'checking'
        : agentLimits
          ? `${records.length} accounts / ${okCount} ok`
          : detected === records.length
            ? `${records.length} available`
            : `${records.length} accounts`;
      queueAgentState.title = detected === records.length ? '' : `${detected} commands detected`;
      queueAgentState.className = records.length && (!agentLimits || okCount > 0) ? 'badge ready' : 'badge warn';
      if (selectedQueueAgent) localStorage.setItem(queueAgentStorageKey, selectedQueueAgent);
    }

    function queueStatusText(item) {
      const view = queueItemView(item);
      if (view.status === 'starting') return 'starting';
      if (view.status === 'running') return 'running';
      if (view.status === 'success') return 'done';
      if (view.status === 'blocked') return 'blocked';
      if (view.status === 'stopped') return 'stopped';
      if (view.status === 'failed') return 'failed';
      return 'waiting';
    }

    function queueItemView(item) {
      const task = taskRecordForQueueItem(item);
      const agentId = item.agentId || task?.agent_id || '';
      const activeAgentId = activeQueueAgentId(item, task);
      if (task?.status === 'closed:success') {
        return {
          status: 'success',
          message: 'closed successfully',
          detail: queueItemDetail(item, task, agentId),
        };
      }
      if (task?.status === 'closed:blocked') {
        return {
          status: 'blocked',
          message: task.status,
          detail: queueItemDetail(item, task, agentId),
        };
      }
      if (task?.status?.startsWith('closed')) {
        return {
          status: 'failed',
          message: task.status,
          detail: queueItemDetail(item, task, agentId),
        };
      }
      if (task?.status === 'open') {
        if (!activeAgentId) {
          return {
            status: item.status === 'starting' ? 'starting' : 'pending',
            message: 'task record open; ready to resume',
            detail: queueItemDetail(item, task, agentId),
          };
        }
        return {
          status: 'running',
          message: agentBadgeText(activeAgentId, task),
          detail: queueItemDetail(item, task, agentId),
        };
      }
      if (activeAgentId) {
        return {
          status: item.status === 'starting' ? 'starting' : 'running',
          message: agentBadgeText(activeAgentId, task) || 'agent running',
          detail: queueItemDetail(item, task, agentId),
        };
      }
      if (agentId && ['starting', 'running'].includes(item.status)) {
        return {
          status: 'failed',
          message: 'agent exited before task closeout',
          detail: queueItemDetail(item, task, agentId),
        };
      }
      return {
        status: item.status || 'pending',
        message: item.message || item.slug || item.prompt.trim().slice(0, 120) || 'empty prompt',
        detail: queueItemDetail(item, task, agentId),
      };
    }

    function queueItemDetail(item, task, agentId) {
      const parts = [];
      const repo = queueItemRepository(item);
      if (repo?.name) parts.push(repo.name);
      if (item.slug) parts.push(`task/${item.slug}`);
      if (task?.status) parts.push(task.status);
      const agentText = agentBadgeText(agentId, task);
      if (agentText) parts.push(agentText);
      if (item.agentCommand) parts.push(item.agentCommand);
      return parts.join(' / ');
    }

    function syncQueueFromSnapshot() {
      if (state?.queue?.run || state?.queue?.records?.length) {
        queueItems = (state.queue.records || []).map(queueItemFromServer);
        queueRun = {
          running: Boolean(state.queue.running),
          stop: false,
          activeIndex: Number(state.queue.run?.current_index ?? -1),
          runId: state.queue.run?.id || existingQueueRunId(),
        };
        return;
      }
      if (!queueTaskRecords().length) return;
      let changed = false;
      for (const item of queueItems) {
        const view = queueItemView(item);
        const task = taskRecordForQueueItem(item);
        if (task?.agent_id && item.agentId !== task.agent_id) {
          item.agentId = task.agent_id;
          changed = true;
        }
        if (view.status !== item.status) {
          item.status = view.status;
          changed = true;
        }
        if (view.message && view.message !== item.message) {
          item.message = view.message;
          changed = true;
        }
      }
      if (changed) saveQueueStorage();
    }

    function queueItemFromServer(item) {
      return {
        ...defaultQueueItem(),
        id: item.id,
        runId: item.run_id || '',
        prompt: item.prompt || '',
        slug: item.slug || '',
        agentId: item.agent_id || '',
        agentCommand: item.agent_command || '',
        repoRoot: item.repo_root || '',
        repoName: item.repo_name || '',
        status: item.status || 'pending',
        message: item.message || '',
        startedAt: item.started_at || 0,
        updatedAt: item.updated_at || 0,
        attempts: item.attempts || 0,
        nextAttemptAt: item.next_attempt_at || 0,
      };
    }

    function renderQueue() {
      document.getElementById('nav-queue').textContent = String(queueItems.length);
      queueState.textContent = queueRun.running ? `running ${queueRun.activeIndex + 1}/${queueItems.length}` : 'idle';
      queueState.className = queueRun.running ? 'badge open' : 'badge warn';
      queueInput.disabled = queueRun.running;
      renderQueueRepoSelector();
      renderQueueAgentSelector();
      document.getElementById('add-queue-task').disabled = queueRun.running || !queueInput.value.trim();
      document.getElementById('clear-queue').disabled = !queueItems.length;
      document.getElementById('run-queue').disabled = queueRun.running || !queueItems.length || !selectedQueueAgentRecord();
      document.getElementById('run-queue').classList.toggle('visible', Boolean(queueItems.length));
      if (!queueItems.length) {
        const empty = document.createElement('div');
        empty.className = 'empty';
        empty.textContent = 'No queued tasks.';
        queueStatus.replaceChildren(empty);
        document.getElementById('stop-queue').classList.toggle('visible', queueRun.running);
        return;
      }
      queueStatus.replaceChildren(...queueItems.map((item, index) => {
        const view = queueItemView(item);
        const node = document.createElement('div');
        node.className = `queue-step ${view.status}`;
        const title = document.createElement('strong');
        title.textContent = `#${index + 1}`;
        const statusNode = badge(queueStatusText(item));
        const message = document.createElement('span');
        message.className = 'queue-step-message';
        const main = document.createElement('span');
        main.textContent = view.message;
        message.appendChild(main);
        if (view.detail) {
          const detail = document.createElement('small');
          detail.textContent = view.detail;
          message.appendChild(detail);
        }
        const controls = queueItemControls(index);
        node.append(title, statusNode, message, controls);
        return node;
      }));
      document.getElementById('stop-queue').classList.toggle('visible', queueRun.running);
    }

    function addQueueTask() {
      const prompt = queueInput.value.trim();
      if (!prompt || queueRun.running) return;
      queueItems.push({ ...defaultQueueItem(), prompt });
      queueInput.value = '';
      saveQueueStorage();
      renderQueue();
      queueInput.focus();
    }

    function queueItemControls(index) {
      const controls = document.createElement('div');
      controls.className = 'queue-step-actions';
      const up = queueActionButton('↑', () => moveQueueItem(index, -1), 'Move task up');
      up.disabled = queueRun.running || index === 0;
      const down = queueActionButton('↓', () => moveQueueItem(index, 1), 'Move task down');
      down.disabled = queueRun.running || index === queueItems.length - 1;
      const open = queueActionButton('↗', () => openQueueItemContext(index), 'Open task context');
      open.disabled = !queueItemContextTarget(queueItems[index]);
      const copy = queueActionButton('', () => copyQueuePrompt(index), 'Copy prompt');
      copy.classList.add('icon-copy');
      const remove = queueActionButton('×', () => removeQueueItem(index), 'Remove');
      remove.classList.add('danger', 'icon-remove');
      remove.disabled = queueRun.running;
      controls.append(up, down, open, copy, remove);
      return controls;
    }

    function queueActionButton(label, action, title = label) {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'secondary compact';
      button.textContent = label;
      button.title = title;
      button.setAttribute('aria-label', title);
      button.addEventListener('click', action);
      return button;
    }

    function moveQueueItem(index, delta) {
      const next = index + delta;
      if (next < 0 || next >= queueItems.length || queueRun.running) return;
      const [item] = queueItems.splice(index, 1);
      queueItems.splice(next, 0, item);
      saveQueueStorage();
      renderQueue();
    }

    function removeQueueItem(index) {
      if (queueRun.running) return;
      const item = queueItems[index];
      const task = taskRecordForQueueItem(item);
      if (item?.runId) {
        removeServerQueueItem(item, task);
        return;
      }
      queueItems.splice(index, 1);
      saveQueueStorage();
      renderQueue();
    }

    async function removeServerQueueItem(item, task) {
      try {
        const response = await fetch('/api/queue/remove', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            run_id: item.runId,
            item_id: item.id,
            task_id: task?.id || (item.slug ? `task/${item.slug}` : ''),
            agent_id: item.agentId || task?.agent_id || '',
          }),
        });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) {
          appendLocalMessage('error', payload.output || 'failed to remove queue item');
          return;
        }
        await loadSnapshot();
      } catch (err) {
        appendLocalMessage('error', String(err));
      }
    }

    async function clearQueue() {
      if (!queueItems.length) return;
      const runId = queueRun.runId || queueItems.find((item) => item.runId)?.runId || '';
      if (runId) {
        try {
          const response = await fetch('/api/queue/clear', {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ run_id: runId }),
          });
          const payload = await response.json().catch(() => ({}));
          if (!response.ok || payload.ok === false) {
            appendLocalMessage('error', payload.output || 'failed to clear queue');
            return;
          }
        } catch (err) {
          appendLocalMessage('error', String(err));
          return;
        }
      }
      queueItems = [];
      queueRun = { running: false, stop: false, activeIndex: -1, runId: '' };
      saveQueueStorage();
      await loadSnapshot();
      renderQueue();
    }

    async function copyQueuePrompt(index) {
      const text = queueItems[index]?.prompt || '';
      if (!text) return;
      await navigator.clipboard.writeText(text);
      if (tg) tg.showAlert('Prompt copied');
    }

    function openQueueItemContext(index) {
      const item = queueItems[index];
      if (!item) return;
      const target = queueItemContextTarget(item);
      if (target?.kind === 'transcript') {
        openTaskTranscript(target.task.id, { terminal: target.terminal });
        return;
      }
      if (target?.kind === 'terminal-chat') {
        openTaskTranscript(target.task.id, { terminal: target.terminal });
        return;
      }
      if (target?.kind === 'task-card') {
        setActiveView('tasks');
        window.setTimeout(() => {
          if (!focusDashboardNode(`.task-record-card[data-task-id="${cssEscape(target.task.id)}"]`)) {
            item.message = 'task record is not visible yet';
            item.updatedAt = Math.floor(Date.now() / 1000);
            saveQueueStorage();
            renderQueue();
          }
        }, 0);
        return;
      }
      if (target?.kind === 'tasks') {
        setActiveView('tasks');
        item.message = 'task record is not available yet';
        item.updatedAt = Math.floor(Date.now() / 1000);
        saveQueueStorage();
        renderQueue();
        return;
      }
      item.message = 'task context is not available yet';
      item.updatedAt = Math.floor(Date.now() / 1000);
      saveQueueStorage();
      renderQueue();
    }

    function queueItemContextTarget(item) {
      if (!item) return null;
      const task = taskRecordForQueueItem(item);
      if (task?.id && task.session_path) {
        return { kind: 'transcript', task, terminal: terminalForQueueItem(item, task) };
      }
      const terminal = terminalForQueueItem(item, task);
      if (task?.id && terminal) {
        return { kind: 'terminal-chat', task, terminal };
      }
      if (task?.id) {
        return { kind: 'task-card', task };
      }
      if (item.slug || item.prompt?.trim()) {
        return { kind: 'tasks' };
      }
      return null;
    }

    function terminalForQueueItem(item, task = taskRecordForQueueItem(item)) {
      const agentId = item.agentId || task?.agent_id || '';
      if (!agentId) return null;
      return (model?.terminals?.records || []).find((terminal) => terminal.agent_id === agentId) || null;
    }

    function focusDashboardNode(selector) {
      const node = document.querySelector(selector);
      if (!node) return false;
      node.scrollIntoView({ behavior: 'smooth', block: 'center' });
      node.classList.remove('dashboard-focus');
      void node.offsetWidth;
      node.classList.add('dashboard-focus');
      window.setTimeout(() => node.classList.remove('dashboard-focus'), 2400);
      return true;
    }

    async function openTaskTranscript(taskId, options = {}) {
      if (!taskId) return;
      const terminal = options.terminal || terminalForTaskId(taskId);
      transcriptContext = {
        taskId,
        terminalTarget: terminal?.target || '',
        chatAvailable: Boolean(terminal?.target),
      };
      transcriptTitle.textContent = 'Task Chat';
      transcriptSubtitle.textContent = taskId;
      renderTranscriptComposer();
      transcriptLog.replaceChildren(Object.assign(document.createElement('div'), {
        className: 'empty',
        textContent: 'Loading transcript.',
      }));
      transcriptModal.hidden = false;
      try {
        const response = await fetch(`/api/task-transcript?id=${encodeURIComponent(taskId)}`, { cache: 'no-store' });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) {
          renderTranscriptFallback(payload.output || 'Transcript is not available.');
          return;
        }
        transcriptTitle.textContent = payload.title || payload.task_id || 'Task Chat';
        transcriptSubtitle.textContent = [
          payload.task_id,
          payload.status,
          payload.session_path || (terminal ? `agent ${terminal.agent_id || terminal.label || terminal.target}` : ''),
        ].filter(Boolean).join(' / ');
        transcriptContext.chatAvailable = Boolean(transcriptContext.terminalTarget || payload.chat_available);
        renderTranscriptComposer();
        if (!transcriptContext.terminalTarget && payload.chat_available) ensureTaskChatTarget(taskId);
        const messages = payload.messages || [];
        if (!messages.length) {
          renderTranscriptFallback('No chat messages found in transcript.');
          return;
        }
        const nodes = messages.map((entry) => messageNode({
          timestamp: Date.parse(entry.timestamp) ? Math.floor(Date.parse(entry.timestamp) / 1000) : 0,
          source: 'task',
          role: entry.role || 'assistant',
          text: entry.text || '',
        }));
        const liveOutput = transcriptTerminalOutputNode(terminal);
        if (liveOutput) nodes.push(liveOutput);
        transcriptLog.replaceChildren(...nodes);
        transcriptLog.scrollTop = transcriptLog.scrollHeight;
      } catch (err) {
        transcriptLog.replaceChildren(Object.assign(document.createElement('div'), {
          className: 'empty',
          textContent: String(err),
        }));
      }
    }

    function renderTranscriptFallback(message) {
      const terminal = terminalForTarget(transcriptContext.terminalTarget);
      const wrap = transcriptTerminalOutputNode(terminal);
      if (wrap) {
        transcriptLog.replaceChildren(wrap);
        transcriptLog.scrollTop = transcriptLog.scrollHeight;
        return;
      }
      transcriptLog.replaceChildren(Object.assign(document.createElement('div'), {
        className: 'empty',
        textContent: transcriptContext.chatAvailable
          ? `${message} Send a message below to continue in the live terminal.`
          : message,
      }));
    }

    function transcriptTerminalOutputNode(terminal) {
      if (!terminal?.output) return null;
      const wrap = document.createElement('div');
      wrap.className = 'terminal-output transcript-terminal-output';
      wrap.tabIndex = 0;
      wrap.addEventListener('keydown', (event) => handleTerminalKeyboard(event, terminal.target));
      renderAnsi(wrap, terminal.output);
      return wrap;
    }

    function renderTranscriptComposer() {
      const enabled = Boolean(transcriptContext.terminalTarget || transcriptContext.chatAvailable);
      transcriptCompose.hidden = !enabled;
      transcriptInput.disabled = !enabled;
      transcriptSend.disabled = !enabled;
      transcriptInput.placeholder = enabled ? 'Message this task agent' : 'No active task terminal';
    }

    function terminalForTaskId(taskId) {
      const task = queueTaskRecords().find((record) => record.id === taskId);
      const queueItem = queueItems.find((item) => `task/${item.slug}` === taskId);
      const agentId = task?.agent_id || queueItem?.agentId || '';
      if (!agentId) return null;
      return (model?.terminals?.records || []).find((terminal) => terminal.agent_id === agentId) || null;
    }

    function terminalForTarget(target) {
      if (!target) return null;
      return (model?.terminals?.records || []).find((terminal) => terminal.target === target) || null;
    }

    async function sendTranscriptMessage() {
      const text = transcriptInput.value.trimEnd();
      if (!text.trim() || !transcriptContext.chatAvailable) return;
      transcriptInput.value = '';
      const payload = await postTaskChatMessage(transcriptContext.taskId, transcriptContext.terminalTarget, text);
      if (!payload.ok) {
        transcriptLog.appendChild(messageNode({
          timestamp: Math.floor(Date.now() / 1000),
          source: 'task',
          role: 'error',
          text: payload.output || 'failed to send task message',
        }));
        transcriptLog.scrollTop = transcriptLog.scrollHeight;
        return;
      }
      if (payload.target) transcriptContext.terminalTarget = payload.target;
      window.setTimeout(async () => {
        await loadSnapshot();
        if (!transcriptModal.hidden && transcriptContext.taskId) {
          openTaskTranscript(transcriptContext.taskId, {
            terminal: terminalForTarget(transcriptContext.terminalTarget),
          });
        }
      }, 350);
    }

    async function postTaskChatMessage(taskId, target, text) {
      const response = await fetch('/api/task-chat/send', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ task_id: taskId, target, text }),
      });
      return response.json().catch(() => ({
        ok: false,
        output: response.ok ? 'invalid task chat response' : `HTTP ${response.status}`,
      }));
    }

    async function ensureTaskChatTarget(taskId) {
      try {
        const response = await fetch('/api/task-chat/target', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ task_id: taskId }),
        });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) return;
        if (transcriptContext.taskId !== taskId) return;
        if (payload.target) transcriptContext.terminalTarget = payload.target;
        transcriptContext.chatAvailable = true;
        renderTranscriptComposer();
        await loadSnapshot();
      } catch (_) {
        // The send path reports startup failures; opening the transcript should stay readable.
      }
    }

    function closeTaskTranscript() {
      transcriptModal.hidden = true;
      transcriptContext = { taskId: '', terminalTarget: '', chatAvailable: false };
      transcriptInput.value = '';
      transcriptLog.replaceChildren();
      renderTranscriptComposer();
    }

    function formatNumber(value) {
      return new Intl.NumberFormat('en-US').format(Number(value || 0));
    }

    function formatTime(unix) {
      if (!unix) return 'unknown';
      return new Date(unix * 1000).toLocaleString();
    }

    const ansiPalette = [
      '#0b1020', '#ff6b6b', '#43c59e', '#f0b35d',
      '#58a6ff', '#d779ff', '#36c5f0', '#d8dee9',
      '#5f6b7a', '#ff8585', '#55d6ad', '#ffd166',
      '#7aa7ff', '#e8a6ff', '#5ddcff', '#ffffff',
    ];

    function ansiIndexedColor(index) {
      if (index < 16) return ansiPalette[index];
      if (index >= 16 && index <= 231) {
        const n = index - 16;
        const r = Math.floor(n / 36);
        const g = Math.floor((n % 36) / 6);
        const b = n % 6;
        const scale = (value) => value === 0 ? 0 : 55 + value * 40;
        return `rgb(${scale(r)}, ${scale(g)}, ${scale(b)})`;
      }
      if (index >= 232 && index <= 255) {
        const level = 8 + (index - 232) * 10;
        return `rgb(${level}, ${level}, ${level})`;
      }
      return null;
    }

    function applyAnsiCode(state, codes, index) {
      const code = codes[index];
      if (code === 0) {
        state.bold = false;
        state.dim = false;
        state.italic = false;
        state.underline = false;
        state.inverse = false;
        state.fg = null;
        state.bg = null;
      } else if (code === 1) state.bold = true;
      else if (code === 2) state.dim = true;
      else if (code === 3) state.italic = true;
      else if (code === 4) state.underline = true;
      else if (code === 7) state.inverse = true;
      else if (code === 22) {
        state.bold = false;
        state.dim = false;
      } else if (code === 23) state.italic = false;
      else if (code === 24) state.underline = false;
      else if (code === 27) state.inverse = false;
      else if (code >= 30 && code <= 37) state.fg = ansiIndexedColor(code - 30);
      else if (code === 39) state.fg = null;
      else if (code >= 40 && code <= 47) state.bg = ansiIndexedColor(code - 40);
      else if (code === 49) state.bg = null;
      else if (code >= 90 && code <= 97) state.fg = ansiIndexedColor(code - 90 + 8);
      else if (code >= 100 && code <= 107) state.bg = ansiIndexedColor(code - 100 + 8);
      else if ((code === 38 || code === 48) && codes[index + 1] === 5) {
        const color = ansiIndexedColor(codes[index + 2]);
        if (code === 38) state.fg = color;
        else state.bg = color;
        return index + 2;
      } else if ((code === 38 || code === 48) && codes[index + 1] === 2) {
        const r = codes[index + 2];
        const g = codes[index + 3];
        const b = codes[index + 4];
        if ([r, g, b].every((value) => Number.isInteger(value) && value >= 0 && value <= 255)) {
          const color = `rgb(${r}, ${g}, ${b})`;
          if (code === 38) state.fg = color;
          else state.bg = color;
        }
        return index + 4;
      }
      return index;
    }

    function applyAnsiStyle(node, state) {
      const fg = state.inverse ? state.bg : state.fg;
      const bg = state.inverse ? state.fg : state.bg;
      if (fg) node.style.color = fg;
      if (bg) node.style.backgroundColor = bg;
      if (state.inverse && !bg) node.style.backgroundColor = '#d8dee9';
      if (state.inverse && !fg) node.style.color = '#0b1020';
      if (state.bold) node.style.fontWeight = '700';
      if (state.dim) node.style.opacity = '.7';
      if (state.italic) node.style.fontStyle = 'italic';
      if (state.underline) node.style.textDecoration = 'underline';
    }

    function renderAnsi(container, text) {
      container.replaceChildren();
      const state = { fg: null, bg: null, bold: false, dim: false, italic: false, underline: false, inverse: false };
      let buffer = '';
      const flush = () => {
        if (!buffer) return;
        const styled = state.fg || state.bg || state.bold || state.dim || state.italic || state.underline || state.inverse;
        if (!styled) {
          container.appendChild(document.createTextNode(buffer));
        } else {
          const span = document.createElement('span');
          span.textContent = buffer;
          applyAnsiStyle(span, state);
          container.appendChild(span);
        }
        buffer = '';
      };

      for (let i = 0; i < text.length; i += 1) {
        if (text[i] !== '\x1b') {
          if (text[i] !== '\x07') buffer += text[i];
          continue;
        }
        if (text[i + 1] === '[') {
          const final = text.slice(i + 2).search(/[\x40-\x7e]/);
          if (final === -1) break;
          flush();
          const finalIndex = i + 2 + final;
          const command = text[finalIndex];
          if (command === 'm') {
            const raw = text.slice(i + 2, finalIndex);
            const codes = raw === '' ? [0] : raw.split(/[;:]/).map((part) => part === '' ? 0 : Number(part));
            for (let codeIndex = 0; codeIndex < codes.length; codeIndex += 1) {
              codeIndex = applyAnsiCode(state, codes, codeIndex);
            }
          }
          i = finalIndex;
        } else if (text[i + 1] === ']') {
          flush();
          const bel = text.indexOf('\x07', i + 2);
          const st = text.indexOf('\x1b\\', i + 2);
          if (bel === -1 && st === -1) break;
          if (bel !== -1 && (st === -1 || bel < st)) i = bel;
          else i = st + 1;
        } else {
          flush();
          i += 1;
        }
      }
      flush();
    }

    function renderTasks() {
      const snapshot = model.taskRecords;
      const items = snapshot.records || [];
      const openItems = items.filter((task) => task.status === 'open');
      const historyItems = items.filter((task) => task.status !== 'open');
      document.getElementById('open-count').textContent = `${openItems.length} open`;
      document.getElementById('failed-count').textContent = `${snapshot.failed || 0} failed`;
      document.getElementById('nav-tasks').textContent = String(snapshot.count || 0);
      renderTaskStats(snapshot);
      if (snapshot.error) {
        const empty = document.createElement('div');
        empty.className = 'empty';
        empty.textContent = snapshot.error;
        tasks.replaceChildren(empty);
        return;
      }
      if (!items.length) {
        tasks.innerHTML = '<div class="empty">No task records yet.</div>';
        return;
      }
      tasks.replaceChildren(
        taskSection('Active Records', `${openItems.length} currently open`, openItems),
        taskSection('History', `${historyItems.length} previous records`, historyItems),
      );
    }

    function renderTaskStats(snapshot) {
      const stats = document.getElementById('task-stats');
      const records = snapshot.records || [];
      const closed = records.filter((task) => task.status.startsWith('closed'));
      const withTelemetry = records.filter((task) => task.token_usage);
      const closedWithTelemetry = closed.filter((task) => task.token_usage);
      const lastDayCutoff = Math.floor(Date.now() / 1000) - 86400;
      const lastDay = records.filter((task) => (task.updated_at || 0) >= lastDayCutoff);
      const lastDayTokens = sumTokens(lastDay);
      const averageClosedTokens = closedWithTelemetry.length
        ? Math.round(sumTokens(closedWithTelemetry) / closedWithTelemetry.length)
        : 0;
      const values = [
        ['Records', snapshot.count || 0],
        ['Previous', records.length - (snapshot.open || 0)],
        ['Closed', closed.length],
        ['With telemetry', withTelemetry.length],
        ['Displayed tokens', formatNumber(snapshot.total_displayed_tokens || 0)],
        ['Avg closed tokens', formatNumber(averageClosedTokens)],
        ['Last 24h records', lastDay.length],
        ['Last 24h tokens', formatNumber(lastDayTokens)],
        ['Output tokens', formatNumber(snapshot.total_output_tokens || 0)],
        ['Reasoning', formatNumber(snapshot.total_reasoning_tokens || 0)],
        ['Tool output tokens', formatNumber(snapshot.total_tool_output_tokens || 0)],
        ['Large outputs', formatNumber(snapshot.total_large_tool_outputs || 0)],
      ];
      stats.replaceChildren(...values.map(([label, value]) => {
        const node = document.createElement('div');
        node.className = 'task-stat';
        const valueNode = document.createElement('strong');
        valueNode.textContent = value;
        const labelNode = document.createElement('span');
        labelNode.textContent = label;
        node.append(valueNode, labelNode);
        return node;
      }));
    }

    function taskSection(title, subtitle, items) {
      const section = document.createElement('section');
      section.className = 'task-section';
      const head = document.createElement('div');
      head.className = 'task-section-head';
      const heading = document.createElement('h3');
      heading.textContent = title;
      const summary = document.createElement('span');
      summary.className = 'badge';
      summary.textContent = subtitle;
      head.append(heading, summary);
      const list = document.createElement('div');
      list.className = 'task-list';
      if (items.length) {
        list.replaceChildren(...items.map(taskCard));
      } else {
        const empty = document.createElement('div');
        empty.className = 'empty';
        empty.textContent = title === 'Active Records' ? 'No open task records.' : 'No previous task records yet.';
        list.append(empty);
      }
      section.append(head, list);
      return section;
    }

    function taskCard(task) {
      const node = document.createElement('article');
      node.className = 'task-card task-record-card';
      node.dataset.taskId = task.id;
      if (task.agent_id) node.dataset.agentId = task.agent_id;
      const title = document.createElement('div');
      const titleText = document.createElement('div');
      titleText.className = 'task-title';
      titleText.textContent = task.title || task.id;
      title.appendChild(titleText);
      if (task.description) title.appendChild(taskDescriptionNode(task.description));
      const path = document.createElement('div');
      path.className = 'task-path';
      path.textContent = task.cwd || task.repo_root || task.session_path || '';
      title.appendChild(path);
      const taskTerminal = terminalForTaskId(task.id);
      if (task.session_path || taskTerminal) {
        const actions = document.createElement('div');
        actions.className = 'task-card-actions';
        actions.appendChild(queueActionButton('Open chat', () => openTaskTranscript(task.id, {
          terminal: terminalForTaskId(task.id),
        }), 'Open task chat'));
        title.appendChild(actions);
      }
      const stateCell = document.createElement('div');
      stateCell.appendChild(badge(task.status));
      const meta = document.createElement('div');
      meta.className = 'task-meta-stack';
      meta.appendChild(badge(task.source || 'task'));
      if (task.sequence) meta.appendChild(badge(`#${String(task.sequence).padStart(6, '0')}`));
      const agentText = agentBadgeText(task.agent_id, task);
      if (agentText) meta.appendChild(badge(agentText));
      if (task.agent_track && task.agent_track !== task.agent_label) meta.appendChild(badge(task.agent_track));
      if (task.kind) meta.appendChild(badge(task.kind));
      const usage = document.createElement('div');
      usage.className = 'task-usage';
      const tokenUsage = task.token_usage;
      usage.innerHTML = tokenUsage
        ? `<strong>${formatNumber(tokenUsage.displayed_total_tokens)}</strong><span>tokens</span><small>${formatNumber(tokenUsage.output_tokens)} out / ${formatNumber(tokenUsage.reasoning_output_tokens)} reasoning</small>`
        : '<strong>-</strong><span>tokens</span><small>no telemetry</small>';
      const efficiency = task.token_efficiency;
      if (efficiency) {
        const detail = document.createElement('small');
        detail.textContent = `${formatNumber(efficiency.tool_output_original_tokens)} tool output / ${formatNumber(efficiency.large_tool_output_calls)} large`;
        usage.appendChild(detail);
      }
      const dates = document.createElement('div');
      dates.className = 'task-path';
      dates.textContent = `created ${formatTime(task.created_at)} / updated ${formatTime(task.updated_at)}`;
      meta.appendChild(dates);
      node.append(title, stateCell, meta, usage);
      return node;
    }

    function compactTaskDescription(description) {
      const text = String(description || '').split(/\s+/).filter(Boolean).join(' ');
      if (text.length <= 180) return text;
      return `${text.slice(0, 177).trimEnd()}...`;
    }

    function taskDescriptionNode(description) {
      const wrap = document.createElement('details');
      wrap.className = 'task-description';
      const summary = document.createElement('summary');
      const preview = document.createElement('span');
      preview.className = 'task-description-preview';
      preview.textContent = compactTaskDescription(description);
      const hint = document.createElement('span');
      hint.className = 'task-description-hint';
      hint.textContent = String(description).length > 180 ? 'Full prompt' : 'Prompt';
      summary.append(preview, hint);
      const full = document.createElement('pre');
      full.className = 'task-description-full';
      full.textContent = description;
      wrap.append(summary, full);
      return wrap;
    }

    function sumTokens(records) {
      return records
        .map((task) => task.token_usage?.displayed_total_tokens || 0)
        .reduce((sum, value) => sum + value, 0);
    }

    function renderAgents() {
      const data = model.agents;
      const host = model.hostAgents;
      const available = model.availableAgents;
      const hostRecords = host.records || [];
      const hostAgentCount = hostRecords.filter((agent) => agent.kind !== 'meta-agent').length;
      const daemonCount = hostRecords.length - hostAgentCount;
      document.getElementById('agent-count').textContent = `${data.runningCount} running`;
      document.getElementById('host-agent-count').textContent = daemonCount
        ? `${hostAgentCount} host / ${daemonCount} daemon`
        : `${hostAgentCount} host`;
      document.getElementById('nav-agents').textContent = String(Math.max(data.runningCount, hostAgentCount, available.count || 0));
      renderAvailableAgents();
      if (!data.records.length) {
        agentList.innerHTML = '<div class="empty">No running Q-COLD agents.</div>';
      } else {
        agentList.replaceChildren(...data.records.map((agent) => {
          const node = document.createElement('article');
          node.className = 'task-card';
          const title = document.createElement('div');
          title.innerHTML = '<div class="task-title"></div><div class="task-path"></div>';
          title.children[0].textContent = agentLabelForId(agent.id) || agent.id;
          title.children[1].textContent = agent.meta.cmd || '';
          const trackCell = document.createElement('div');
          trackCell.appendChild(badge(agent.meta.track || 'track'));
          trackCell.appendChild(badge(shortAgentId(agent.id)));
          const stateCell = document.createElement('div');
          stateCell.appendChild(badge(agent.meta.state || 'unknown'));
          node.append(title, trackCell, stateCell);
          return node;
        }));
      }

      if (!host.records.length) {
        hostAgentList.innerHTML = '<div class="empty">No host agent processes discovered.</div>';
        return;
      }
      hostAgentList.replaceChildren(...host.records.map((agent) => {
        const node = document.createElement('article');
        node.className = 'task-card';
        const title = document.createElement('div');
        title.innerHTML = '<div class="task-title"></div><div class="task-path"></div>';
        title.children[0].textContent = `${agent.kind} pid=${agent.pid}`;
        title.children[1].textContent = agent.command || '';
        const kindCell = document.createElement('div');
        kindCell.appendChild(badge(agent.kind || 'process'));
        const cwdCell = document.createElement('div');
        cwdCell.className = 'task-path';
        cwdCell.textContent = agent.cwd || 'unknown';
        node.append(title, kindCell, cwdCell);
        return node;
      }));
    }

    function renderAvailableAgents() {
      const records = model.availableAgents.records || [];
      const limits = new Map((agentLimits?.records || []).map((record) => [record.command, record]));
      const stateText = agentLimitsLoading
        ? 'checking'
        : agentLimits
          ? `${agentLimits.count} checked${agentLimits.cached ? ' / cached' : ''}`
          : 'not checked';
      agentLimitState.textContent = stateText;
      agentLimitState.className = agentLimitsLoading ? 'badge open' : agentLimits ? 'badge ready' : 'badge warn';
      if (!records.length) {
        availableAgentList.innerHTML = '<div class="empty">No local agent commands found in PATH.</div>';
        return;
      }
      availableAgentList.replaceChildren(...records.map((agent) => availableAgentCard(agent, limits.get(agent.command))));
    }

    function availableAgentCard(agent, limit) {
      const node = document.createElement('article');
      node.className = 'agent-command-card';
      const title = document.createElement('div');
      title.innerHTML = '<div class="task-title"></div><div class="task-path"></div>';
      title.children[0].textContent = `${agent.command} - ${agent.label}`;
      title.children[1].textContent = agent.path || '';
      const meta = document.createElement('div');
      meta.className = 'task-meta-stack';
      meta.appendChild(badge(`acct ${agent.account || 'default'}`));
      meta.appendChild(badge(agent.invocation || 'agent'));
      const status = document.createElement('div');
      status.className = 'agent-limit-status';
      if (limit) {
        status.appendChild(limitBadge(limit.state));
        const summary = document.createElement('span');
        summary.textContent = limit.summary || limit.state;
        status.appendChild(summary);
      } else {
        status.appendChild(limitBadge('unknown'));
        const summary = document.createElement('span');
        summary.textContent = 'not checked';
        status.appendChild(summary);
      }
      node.append(title, meta, status);
      return node;
    }

    function limitBadge(state) {
      const span = document.createElement('span');
      const tone = state === 'limited' || state === 'error' || state === 'unauthenticated'
        ? 'failed'
        : state === 'timeout' || state === 'unknown'
          ? 'warn'
          : 'ready';
      span.className = `badge ${tone}`;
      span.textContent = state || 'unknown';
      return span;
    }

    function renderTerminals() {
      const terminals = model.terminals;
      document.getElementById('terminal-count').textContent = `${terminals.count} attachable`;
      document.getElementById('nav-terminals').textContent = String(terminals.count);
      if (!terminals.records.length) {
        terminalList.innerHTML = `<div class="empty">${model.hostAgents.count} host agents detected, but no attachable terminal sessions. Start new agents through Q-COLD so they run in managed terminal sessions.</div>`;
        terminalOutputCache.clear();
        return;
      }
      const targets = new Set(terminals.records.map((terminal) => terminal.target));
      Array.from(terminalList.querySelectorAll('.terminal-card')).forEach((node) => {
        if (!targets.has(node.dataset.target)) {
          terminalOutputCache.delete(node.dataset.target);
          node.remove();
        }
      });
      for (const terminal of terminals.records) {
        let node = terminalList.querySelector(`.terminal-card[data-target="${cssEscape(terminal.target)}"]`);
        if (!node) {
          node = createTerminalCard(terminal);
          terminalList.appendChild(node);
        }
        updateTerminalCard(node, terminal);
        terminalList.appendChild(node);
      }
    }

    function cssEscape(value) {
      if (window.CSS && CSS.escape) return CSS.escape(value);
      return String(value).replace(/["\\]/g, '\\$&');
    }

    function createTerminalCard(terminal) {
      const node = document.createElement('article');
      node.className = 'terminal-card';
      node.dataset.target = terminal.target;
      node.dataset.agentId = terminal.agent_id || '';
      const head = document.createElement('div');
      head.className = 'terminal-head';
      head.innerHTML = '<div data-role="title"></div><span data-role="kind"></span><span data-role="cwd"></span>';
      const output = document.createElement('pre');
      output.className = 'terminal-output';
      output.tabIndex = 0;
      output.addEventListener('keydown', (event) => handleTerminalKeyboard(event, terminal.target));
      const compose = terminalComposer(terminal);
      node.append(head, output, compose);
      return node;
    }

    function updateTerminalCard(node, terminal) {
      node.dataset.agentId = terminal.agent_id || '';
      const head = node.querySelector('.terminal-head');
      const title = head.querySelector('[data-role="title"]');
      const active = document.activeElement;
      if (!title.contains(active)) {
        title.replaceChildren(terminalTitleControl(terminal));
      }
      head.querySelector('[data-role="kind"]').replaceChildren(badge(terminalKind(terminal)));
      head.querySelector('[data-role="cwd"]').textContent = terminal.cwd || '';
      const output = node.querySelector('.terminal-output');
      const nextOutput = terminal.output || '';
      if (terminalOutputCache.get(terminal.target) !== nextOutput) {
        const shouldFollowTail = isTerminalAtTail(output);
        const previousScrollTop = output.scrollTop;
        renderAnsi(output, nextOutput);
        if (shouldFollowTail) {
          output.scrollTop = output.scrollHeight;
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
        wrap.replaceWith(terminalTitleControl(terminal));
      });
      form.addEventListener('submit', async (event) => {
        event.preventDefault();
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
      });
      input.addEventListener('keydown', (event) => {
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
      return compose;
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

    function renderHistory(items) {
      document.getElementById('nav-chat').textContent = String(items.length);
      if (!items.length) {
        chatLog.innerHTML = '<div class="empty">No local chat history yet.</div>';
        return;
      }
      chatLog.replaceChildren(...items.map(messageNode));
      chatLog.scrollTop = chatLog.scrollHeight;
    }

    function appendLocalMessage(role, text, source = 'web') {
      const existingEmpty = chatLog.querySelector('.empty');
      if (existingEmpty) chatLog.replaceChildren();
      chatLog.appendChild(messageNode({
        timestamp: Math.floor(Date.now() / 1000),
        source,
        role,
        text,
      }));
      chatLog.scrollTop = chatLog.scrollHeight;
    }

    function renderSystemStrip() {
      const summary = model.status.summary;
      const terminalReady = summary.terminal_ready === 'yes';
      const openTasks = Number(summary.open_tasks || 0);
      const incomplete = Number(summary.incomplete_closeouts || 0);
      const dirty = Number(summary.primary_dirty || 0);
      document.getElementById('ready-pill').textContent = terminalReady ? 'terminal ready' : 'terminal hold';
      document.getElementById('ready-pill').className = terminalReady ? 'badge ready' : 'badge warn';
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
      document.getElementById('strip-terminal').textContent = terminalReady ? 'Terminal OK' : 'Terminal hold';
      document.getElementById('strip-terminal').className = terminalReady ? 'badge ready' : 'badge warn';
      document.getElementById('strip-repo').textContent = `${state.repository.name} / ${state.repository.branch}`;
      document.getElementById('strip-tasks').textContent = `${open} task records / ${openTasks} worktrees / ${failed} failed${dirty ? ` / ${dirty} dirty` : ''}`;
      document.getElementById('strip-closeouts').textContent = `${incomplete} closeout residue`;
      document.getElementById('strip-closeouts').className = incomplete ? 'strip-text bad' : 'strip-text';
      const hostRecords = model.hostAgents.records || [];
      const hostAgentCount = hostRecords.filter((agent) => agent.kind !== 'meta-agent').length;
      const daemonCount = hostRecords.length - hostAgentCount;
      document.getElementById('strip-agents').textContent = `${model.terminals.count} terminals / ${hostAgentCount} host${daemonCount ? ` / ${daemonCount} daemon` : ''}`;
      document.getElementById('write-state').textContent = 'local write';
      document.getElementById('write-state').className = 'badge ready';
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
      liveState.textContent = label;
      liveState.className = `badge ${tone} live-indicator`;
    }

    function applySnapshot(snapshot) {
      state = snapshot.state;
      render();
      renderHistory(snapshot.history || []);
      setLiveState('Live');
    }

    async function loadSnapshot() {
      try {
        const response = await fetch('/api/state', { cache: 'no-store' });
        const nextState = await response.json();
        const historyResponse = await fetch('/api/history', { cache: 'no-store' });
        applySnapshot({ state: nextState, history: await historyResponse.json() });
      } catch (err) {
        setLiveState('Offline', 'failed');
        if (!state) status.textContent = String(err);
      }
    }

    function connectEvents() {
      if (!window.EventSource) {
        loadSnapshot();
        fallbackTimer = window.setInterval(loadSnapshot, 5000);
        return;
      }
      eventSource = new EventSource('/api/events');
      eventSource.addEventListener('snapshot', (event) => applySnapshot(JSON.parse(event.data)));
      eventSource.addEventListener('error', () => setLiveState('Offline', 'failed'));
      eventSource.onopen = () => setLiveState('Live');
    }

    async function postChatText(text) {
      const response = await fetch('/api/chat', {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
        },
        body: JSON.stringify({ text }),
      });
      const payload = await response.json();
      if (!response.ok && payload.ok !== false) {
        payload.ok = false;
      }
      return payload;
    }

    async function sendChat(text, source = 'web') {
      const trimmed = text.trim();
      if (!trimmed) return;
      appendLocalMessage('operator', trimmed, source);
      if (source === 'web') chatInput.value = '';
      try {
        const payload = await postChatText(trimmed);
        appendLocalMessage(payload.ok ? 'assistant' : 'error', payload.output || 'No output', source);
        return payload;
      } catch (err) {
        appendLocalMessage('error', String(err), source);
        return { ok: false, output: String(err) };
      }
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
      return queueTaskRecords().find((task) => (
        task.id === `task/${item.slug}`
        && (!repo?.root || !task.repo_root || task.repo_root === repo.root)
      ));
    }

    function runningAgent(agentId) {
      if (!agentId || !model) return true;
      return (model.agents.records || []).some((agent) => agent.id === agentId);
    }

    function activeQueueAgentId(item, task = taskRecordForQueueItem(item)) {
      return [item.agentId, task?.agent_id].find((agentId) => agentId && runningAgent(agentId)) || '';
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
      const now = Math.floor(Date.now() / 1000);
      const selectedRepo = selectedQueueRepository();
      queueRun = {
        running: true,
        stop: false,
        activeIndex: -1,
        runId: existingQueueRunId() || Date.now().toString(36),
      };
      const usedSlugs = usedQueueSlugs(queueRun.runId);
      queueItems = queueItems.map((item, index) => {
        const slug = item.slug || nextQueueSlug(queueRun.runId, usedSlugs, index);
        const task = taskRecordForQueueItem(item);
        const repo = item.repoRoot ? queueItemRepository(item) : selectedRepo;
        const closedStatus = task?.status?.startsWith('closed') ? task.status : '';
        const success = closedStatus === 'closed:success' || item.status === 'success';
        const prompt = item.prompt.trim();
        return {
          ...item,
          slug,
          agentId: item.agentId || task?.agent_id || '',
          repoRoot: repo.root || '',
          repoName: repo.name || '',
          status: success ? 'success' : closedStatus ? 'failed' : prompt ? 'pending' : 'failed',
          message: success ? 'closed successfully' : closedStatus || (prompt ? '' : 'empty prompt'),
          agentCommand: item.agentCommand || selectedAgent?.command || '',
          startedAt: item.startedAt || now,
          updatedAt: now,
        };
      });
      saveQueueStorage();
      renderQueue();
      const response = await fetch('/api/queue/run', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          run_id: queueRun.runId,
          selected_agent_command: selectedAgent.command,
          selected_repo_root: selectedRepo.root || '',
          selected_repo_name: selectedRepo.name || '',
          items: queueItems.map((item) => ({
            id: item.id,
            prompt: item.prompt,
            slug: item.slug,
            repo_root: item.repoRoot,
            repo_name: item.repoName,
            agent_command: item.agentCommand || selectedAgent.command,
          })),
        }),
      });
      const payload = await response.json().catch(() => ({}));
      if (!response.ok || payload.ok === false) {
        queueRun.running = false;
        queueItems[0].status = 'failed';
        queueItems[0].message = payload.output || 'failed to start backend queue';
        saveQueueStorage();
        renderQueue();
        return;
      }
      await loadSnapshot();
    }

    async function stopQueue() {
      queueRun.stop = true;
      try {
        await fetch('/api/queue/stop', { method: 'POST' });
        await loadSnapshot();
      } catch (err) {
        appendLocalMessage('error', String(err));
      }
      renderQueue();
    }

    function preferredView() {
      const fromHash = window.location.hash.replace(/^#/, '');
      if (fromHash === 'start') return 'queue';
      if (viewNames.has(fromHash)) return fromHash;
      const stored = localStorage.getItem('qcold-view') || '';
      if (stored === 'start') return 'queue';
      return viewNames.has(stored) ? stored : 'chat';
    }

    function setActiveView(view, persist = true) {
      const next = viewNames.has(view) ? view : 'chat';
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

    document.getElementById('send-chat').addEventListener('click', () => sendChat(chatInput.value));
    document.getElementById('close-transcript').addEventListener('click', closeTaskTranscript);
    transcriptSend.addEventListener('click', sendTranscriptMessage);
    transcriptInput.addEventListener('keydown', (event) => {
      if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') sendTranscriptMessage();
    });
    transcriptModal.addEventListener('click', (event) => {
      if (event.target === transcriptModal) closeTaskTranscript();
    });
    document.getElementById('add-queue-task').addEventListener('click', addQueueTask);
    document.getElementById('clear-queue').addEventListener('click', clearQueue);
    document.getElementById('run-queue').addEventListener('click', runQueue);
    document.getElementById('stop-queue').addEventListener('click', stopQueue);
    document.getElementById('refresh-agent-limits').addEventListener('click', () => loadAgentLimits(true));
    queueInput.addEventListener('keydown', (event) => {
      if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') addQueueTask();
    });
    queueInput.addEventListener('input', renderQueue);
    queueRepoSelect.addEventListener('change', () => {
      selectedQueueRepoRoot = queueRepoSelect.value;
      localStorage.setItem(queueRepoStorageKey, selectedQueueRepoRoot);
      renderQueue();
    });
    queueAgentSelect.addEventListener('change', () => {
      selectedQueueAgent = queueAgentSelect.value;
      localStorage.setItem(queueAgentStorageKey, selectedQueueAgent);
      renderQueue();
    });
    chatInput.addEventListener('keydown', (event) => {
      if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') sendChat(chatInput.value);
    });
    themeButtons.forEach((button) => {
      button.addEventListener('click', () => applyTheme(button.dataset.themeChoice));
    });
    viewButtons.forEach((button) => {
      button.addEventListener('click', () => {
        setActiveView(button.dataset.view);
      });
    });
    applyTheme();
    setActiveView(preferredView(), false);
    renderQueue();
    connectEvents();
    window.addEventListener('hashchange', () => setActiveView(preferredView()));
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) {
        if (eventSource) eventSource.close();
        if (fallbackTimer) window.clearInterval(fallbackTimer);
        fallbackTimer = null;
      } else {
        connectEvents();
      }
    });
