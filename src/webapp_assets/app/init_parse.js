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
    const queueGraphModeInput = document.getElementById('queue-graph-mode');
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
    const terminalTailLocks = new Map();
    const viewButtons = Array.from(document.querySelectorAll('.nav button'));
    const viewNames = new Set(viewButtons.map((button) => button.dataset.view));
    const queueStorageKey = 'qcold-task-queue-v4';
    const queueAgentStorageKey = 'qcold-task-queue-agent-v1';
    const queueRepoStorageKey = 'qcold-task-queue-repo-v1';
    const queueGraphModeStorageKey = 'qcold-task-queue-graph-mode-v1';
    let selectedQueueAgent = localStorage.getItem(queueAgentStorageKey) || '';
    let selectedQueueRepoRoot = localStorage.getItem(queueRepoStorageKey) || '';
    let queueGraphMode = localStorage.getItem(queueGraphModeStorageKey) === '1';
    const queueSaved = loadQueueStorage();
    let queueItems = (queueSaved.items || [])
      .map((item) => ({ ...defaultQueueItem(), ...item }));
    let queueWaves = normalizeQueueWaves(queueSaved.waves || [], queueItems);
    let queueRun = { running: false, stopped: false, stop: false, activeIndex: -1, runId: '', status: '' };
    let transcriptContext = { taskId: '', terminalTarget: '', chatAvailable: false };
    let agentLimits = null;
    let agentLimitsLoading = false;
    const removingQueueItems = new Set();

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
      const tone = status.includes('failed')
        ? 'failed'
        : status === 'open'
          ? 'open'
          : status.includes('blocked') ? 'warn' : 'ready';
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
      if (queueGraphMode) syncQueueWaveDependencies();
      const draftItems = queueItems.filter((item) => !item.runId);
      const draftWaves = normalizeQueueWaves(queueWaves, draftItems);
      if (!draftItems.length && draftWaves.length <= 1) {
        localStorage.removeItem(queueStorageKey);
        return;
      }
      localStorage.setItem(queueStorageKey, JSON.stringify({
        waves: draftWaves.map((wave) => ({ id: wave.id })),
        items: draftItems.map((item) => ({
          id: item.id,
          runId: '',
          prompt: item.prompt,
          slug: '',
          agentId: '',
          agentCommand: item.agentCommand,
          dependsOn: item.dependsOn || [],
          waveId: item.waveId || '',
          gatesNext: item.gatesNext !== false,
          repoRoot: item.repoRoot,
          repoName: item.repoName,
          position: null,
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
        dependsOn: [],
        waveId: '',
        gatesNext: true,
        repoRoot: '',
        repoName: '',
        position: null,
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
      if (view.status === 'idle') return 'idle';
      if (view.status === 'success') return 'done';
      if (view.status === 'blocked') return 'blocked';
      if (view.status === 'stopped') return 'stopped';
      if (view.status === 'paused') return 'paused';
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
      if (queueRun.running && item.status === 'starting' && task?.status === 'paused') {
        return {
          status: 'starting',
          message: item.message || 'continuing queue',
          detail: queueItemDetail(item, task, agentId),
        };
      }
      if (task?.status === 'paused') {
        return {
          status: 'paused',
          message: 'paused; press Continue to resume',
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
      if (queueRun.stopped && ['stopped', 'paused'].includes(item.status)) {
        return {
          status: 'stopped',
          message: item.message || 'stopped by operator; press Continue to resume',
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
        const terminal = terminalForAgentId(activeAgentId);
        if (terminalCloseoutFailure(terminal)) {
          return {
            status: 'failed',
            message: 'agent stopped after failed Q-COLD closeout',
            detail: queueItemDetail(item, task, agentId),
          };
        }
        if (terminalIdlePrompt(terminal)) {
          return {
            status: 'idle',
            message: 'agent idle; task is still open',
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
        const terminal = terminalForAgentId(activeAgentId);
        if (terminalCloseoutFailure(terminal)) {
          return {
            status: 'failed',
            message: 'agent stopped after failed Q-COLD closeout',
            detail: queueItemDetail(item, task, agentId),
          };
        }
        if (terminalIdlePrompt(terminal)) {
          return {
            status: 'idle',
            message: 'agent idle; no task closeout detected',
            detail: queueItemDetail(item, task, agentId),
          };
        }
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
        localStorage.removeItem(queueStorageKey);
        const previousItems = new Map(queueItems.map((item) => [item.id, item]));
        queueItems = (state.queue.records || [])
          .map(queueItemFromServer)
          .map((item) => ({
            ...item,
            gatesNext: previousItems.get(item.id)?.gatesNext ?? item.gatesNext,
          }))
          .filter((item) => !removingQueueItems.has(queueItemKey(item)));
        queueWaves = normalizeQueueWaves(queueWaves, queueItems);
        queueRun = {
          running: Boolean(state.queue.running),
          stopped: state.queue.run?.status === 'stopped',
          stop: false,
          activeIndex: Number(state.queue.run?.current_index ?? -1),
          runId: state.queue.run?.id || existingQueueRunId(),
          status: state.queue.run?.status || '',
        };
        queueGraphMode = state.queue.run?.execution_mode === 'graph';
        return;
      }
      queueRun = { running: false, stopped: false, stop: false, activeIndex: -1, runId: '', status: '' };
      let changed = false;
      const beforeCount = queueItems.length;
      queueItems = queueItems.filter((item) => !isStaleQueueItem(item));
      if (queueItems.length !== beforeCount) {
        changed = true;
      }
      const previousWaves = queueWaves.map((wave) => wave.id).join(',');
      queueWaves = normalizeQueueWaves(queueWaves, queueItems);
      if (previousWaves !== queueWaves.map((wave) => wave.id).join(',')) {
        changed = true;
      }
      if (!queueTaskRecords().length) {
        if (changed) saveQueueStorage();
        return;
      }
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

    function isStaleQueueItem(item) {
      if (!item) return true;
      const task = taskRecordForQueueItem(item);
      if (task?.status?.startsWith('closed')) return true;
      if (item.runId && !task) return true;
      if (item.slug && !task) return true;
      if (item.slug && ['success', 'failed', 'blocked'].includes(item.status)) return true;
      return false;
    }

    function queueItemFromServer(item) {
      return {
        ...defaultQueueItem(),
        id: item.id,
        runId: item.run_id || '',
        prompt: item.prompt || '',
        slug: item.slug || '',
        dependsOn: Array.isArray(item.depends_on) ? item.depends_on : [],
        waveId: '',
        gatesNext: true,
        agentId: item.agent_id || '',
        agentCommand: item.agent_command || '',
        repoRoot: item.repo_root || '',
        repoName: item.repo_name || '',
        position: Number(item.position ?? 0),
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
      queueGraphModeInput.checked = queueGraphMode;
      queueGraphModeInput.disabled = queueHasBackendRun();
      queueState.textContent = queueRun.running
        ? queueRunningText()
        : queueRun.stopped
          ? 'stopped'
          : queueGraphMode ? 'graph' : 'idle';
      queueState.className = queueRun.running ? 'badge open' : 'badge warn';
      queueInput.disabled = false;
      renderQueueRepoSelector();
      renderQueueAgentSelector();
      document.getElementById('add-queue-task').disabled = !queueInput.value.trim();
      document.getElementById('clear-queue').disabled = !queueCanClear();
      const addWaveButton = document.getElementById('add-queue-wave');
      addWaveButton.hidden = !queueGraphMode;
      addWaveButton.disabled = !queueGraphLayoutEditable();
      document.getElementById('run-queue').disabled = queueRun.running
        || queueRun.stopped
        || !queueItems.length
        || !selectedQueueAgentRecord();
      document.getElementById('run-queue').classList.toggle(
        'visible',
        Boolean(queueItems.length) && !queueRun.stopped,
      );
      const stopButton = document.getElementById('stop-queue');
      stopButton.textContent = queueRun.stopped ? 'Continue' : 'Stop';
      stopButton.classList.toggle('visible', queueRun.running || queueRun.stopped);
      if (!queueItems.length && !queueShouldRenderEmptyGraph()) {
        const empty = document.createElement('div');
        empty.className = 'empty';
        empty.textContent = 'No queued tasks.';
        queueStatus.replaceChildren(empty);
        return;
      }
      if (queueGraphMode) {
        renderQueueGraph();
        return;
      }
      queueStatus.replaceChildren(...queueItems.map((item, index) => {
        const view = queueItemView(item);
        const node = document.createElement('div');
        node.className = `queue-step ${view.status}`;
        const title = document.createElement('strong');
