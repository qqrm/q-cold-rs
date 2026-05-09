const tg = window.Telegram && window.Telegram.WebApp;
    if (tg) { tg.ready(); tg.expand(); }

    let state = null;
    let model = null;
    const status = document.getElementById('status');
    const agents = document.getElementById('agents');
    const tasks = document.getElementById('tasks');
    const agentList = document.getElementById('agent-list');
    const hostAgentList = document.getElementById('host-agent-list');
    const terminalList = document.getElementById('terminal-list');
    const queueInput = document.getElementById('queue-input');
    const queueState = document.getElementById('queue-state');
    const queueStatus = document.getElementById('queue-status');
    const chatLog = document.getElementById('chat-log');
    const chatInput = document.getElementById('chat-input');
    const themeButtons = Array.from(document.querySelectorAll('[data-theme-choice]'));
    const liveState = document.getElementById('live-state');
    let fallbackTimer = null;
    let eventSource = null;
    const terminalDrafts = new Map();
    const terminalOutputCache = new Map();
    const viewButtons = Array.from(document.querySelectorAll('.nav button'));
    const viewNames = new Set(viewButtons.map((button) => button.dataset.view));
    const queueStorageKey = 'qcold-task-queue-v4';
    const queueSaved = loadQueueStorage();
    let queueItems = (queueSaved.items || [])
      .map((item) => ({ ...defaultQueueItem(), ...item, status: 'pending', message: '' }));
    let queueRun = { running: false, stop: false, activeIndex: -1, runId: '' };

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

    function loadQueueStorage() {
      try {
        return JSON.parse(localStorage.getItem(queueStorageKey) || '{}');
      } catch (_err) {
        return {};
      }
    }

    function saveQueueStorage() {
      localStorage.setItem(queueStorageKey, JSON.stringify({
        items: queueItems.map((item) => ({ id: item.id, prompt: item.prompt })),
      }));
    }

    function defaultQueueItem() {
      return {
        id: `queue-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
        prompt: '',
        slug: '',
        agentId: '',
        status: 'pending',
        message: '',
      };
    }

    function sanitizeSlug(value) {
      return (value || 'queued-task')
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, '-')
        .replace(/^-+|-+$/g, '')
        .slice(0, 64) || 'queued-task';
    }

    function slugForQueueItem(index, runId = queueRun.runId || 'next') {
      return `task-${runId}-${String(index + 1).padStart(2, '0')}`;
    }

    function queueTrack(runId = queueRun.runId || 'next') {
      return `queue-${sanitizeSlug(runId)}`;
    }

    function shellQuote(value) {
      return `'${String(value).replace(/'/g, `'\\''`)}'`;
    }

    function taskInstruction(item, index) {
      const root = state?.repository?.root || '<repo>';
      const taskSlug = item.slug || slugForQueueItem(index);
      return `Use the launched host-side agent workspace as your home base for ${root}; do not enter a devcontainer from $QCOLD_AGENT_WORKTREE. Start managed task ${taskSlug} with cargo qcold task open ${taskSlug}, enter that managed task worktree and its devcontainer if the task flow provides one, reread AGENTS.md and task logs, then do: ${item.prompt.trim()} Drive the task to terminal closeout unless blocked. After closeout, cd back to $QCOLD_AGENT_WORKTREE before starting a new chat or task.`;
    }

    function queueCommand(item, index) {
      return `/agent_start ${queueTrack()} :: codex exec ${shellQuote(taskInstruction(item, index))}`;
    }

    function queueStatusText(item) {
      if (item.status === 'starting') return 'starting';
      if (item.status === 'running') return 'running';
      if (item.status === 'success') return 'done';
      if (item.status === 'stopped') return 'stopped';
      if (item.status === 'failed') return 'failed';
      return 'waiting';
    }

    function renderQueue() {
      document.getElementById('nav-queue').textContent = String(queueItems.length);
      queueState.textContent = queueRun.running ? `running ${queueRun.activeIndex + 1}/${queueItems.length}` : 'idle';
      queueState.className = queueRun.running ? 'badge open' : 'badge warn';
      queueInput.disabled = queueRun.running;
      document.getElementById('add-queue-task').disabled = queueRun.running || !queueInput.value.trim();
      document.getElementById('run-queue').disabled = queueRun.running || !queueItems.length;
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
        const node = document.createElement('div');
        node.className = `queue-step ${item.status}`;
        const title = document.createElement('strong');
        title.textContent = `Task ${index + 1}`;
        const statusNode = badge(queueStatusText(item));
        const message = document.createElement('span');
        message.textContent = item.message || item.slug || item.prompt.trim().slice(0, 120) || 'empty prompt';
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
      const copy = queueActionButton('Copy prompt', () => copyQueuePrompt(index));
      const remove = queueActionButton('Remove', () => removeQueueItem(index));
      remove.classList.add('danger');
      remove.disabled = queueRun.running;
      controls.append(up, down, copy, remove);
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
      queueItems.splice(index, 1);
      saveQueueStorage();
      renderQueue();
    }

    async function copyQueuePrompt(index) {
      const text = queueItems[index]?.prompt || '';
      if (!text) return;
      await navigator.clipboard.writeText(text);
      if (tg) tg.showAlert('Prompt copied');
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
      const title = document.createElement('div');
      title.innerHTML = '<div class="task-title"></div><div class="task-description"></div><div class="task-path"></div>';
      title.children[0].textContent = task.title || task.id;
      title.children[1].textContent = task.description || '';
      title.children[2].textContent = task.cwd || task.repo_root || task.session_path || '';
      const stateCell = document.createElement('div');
      stateCell.appendChild(badge(task.status));
      const meta = document.createElement('div');
      meta.className = 'task-meta-stack';
      meta.appendChild(badge(task.source || 'task'));
      if (task.sequence) meta.appendChild(badge(`#${String(task.sequence).padStart(6, '0')}`));
      if (task.agent_id) meta.appendChild(badge(`agent ${task.agent_id.slice(0, 8)}`));
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

    function sumTokens(records) {
      return records
        .map((task) => task.token_usage?.displayed_total_tokens || 0)
        .reduce((sum, value) => sum + value, 0);
    }

    function renderAgents() {
      const data = model.agents;
      const host = model.hostAgents;
      const hostRecords = host.records || [];
      const hostAgentCount = hostRecords.filter((agent) => agent.kind !== 'meta-agent').length;
      const daemonCount = hostRecords.length - hostAgentCount;
      document.getElementById('agent-count').textContent = `${data.runningCount} running`;
      document.getElementById('host-agent-count').textContent = daemonCount
        ? `${hostAgentCount} host / ${daemonCount} daemon`
        : `${hostAgentCount} host`;
      document.getElementById('nav-agents').textContent = String(Math.max(data.runningCount, hostAgentCount));
      if (!data.records.length) {
        agentList.innerHTML = '<div class="empty">No running Q-COLD agents.</div>';
      } else {
        agentList.replaceChildren(...data.records.map((agent) => {
          const node = document.createElement('article');
          node.className = 'task-card';
          const title = document.createElement('div');
          title.innerHTML = '<div class="task-title"></div><div class="task-path"></div>';
          title.children[0].textContent = agent.meta.name || agent.id;
          title.children[1].textContent = agent.meta.cmd || '';
          const trackCell = document.createElement('div');
          trackCell.appendChild(badge(agent.meta.track || 'track'));
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
      const head = document.createElement('div');
      head.className = 'terminal-head';
      head.innerHTML = '<div data-role="title"></div><span data-role="kind"></span><span data-role="cwd"></span>';
      const output = document.createElement('pre');
      output.className = 'terminal-output';
      const compose = terminalComposer(terminal);
      node.append(head, output, compose);
      return node;
    }

    function updateTerminalCard(node, terminal) {
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
        renderAnsi(output, nextOutput);
        terminalOutputCache.set(terminal.target, nextOutput);
      }
      const input = node.querySelector('.terminal-input');
      if (input) input.placeholder = `send to ${terminalLabel(terminal)}`;
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
        hostAgents: state.host_agents || { count: 0, records: [] },
        terminals: state.terminals || { count: 0, records: [] },
      };
      status.textContent = state.status.text;
      agents.textContent = state.agents.text;
      renderSystemStrip();
      renderTasks();
      renderAgents();
      renderTerminals();
      renderQueue();
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
      try {
        await fetch('/api/terminal/send', {
          method: 'POST',
          headers: {
            'content-type': 'application/json',
          },
          body: JSON.stringify({ target, text: trimmed }),
        });
        window.setTimeout(loadSnapshot, 250);
      } catch (err) {
        appendLocalMessage('error', String(err));
      }
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

    function parseStartedAgentId(output) {
      const match = String(output || '').match(/^agent\t([^\t\n]+)/m);
      return match ? match[1] : '';
    }

    function taskRecordForQueueItem(item) {
      const records = state?.task_records?.records || [];
      return records.find((task) => task.id === `task/${item.slug}`);
    }

    function runningAgent(agentId) {
      if (!agentId || !model) return true;
      return (model.agents.records || []).some((agent) => agent.id === agentId);
    }

    async function waitForQueueTask(item) {
      for (;;) {
        if (queueRun.stop) return { ok: false, status: 'stopped', message: 'stopped by operator' };
        await new Promise((resolve) => window.setTimeout(resolve, 5000));
        await loadSnapshot();
        const task = taskRecordForQueueItem(item);
        if (task?.status === 'closed:success') {
          return { ok: true, status: task.status, message: task.title || item.slug };
        }
        if (task?.status && task.status.startsWith('closed')) {
          return { ok: false, status: task.status, message: task.title || item.slug };
        }
        if (task?.status === 'open' && item.agentId && !runningAgent(item.agentId)) {
          return { ok: false, status: 'failed', message: 'agent exited before task closeout' };
        }
        if (!task && item.agentId && !runningAgent(item.agentId)) {
          return { ok: false, status: 'failed', message: 'agent exited before opening task record' };
        }
      }
    }

    async function runQueue() {
      if (queueRun.running || !queueItems.length) return;
      queueRun = {
        running: true,
        stop: false,
        activeIndex: -1,
        runId: Date.now().toString(36),
      };
      queueItems = queueItems.map((item, index) => ({
        ...item,
        slug: slugForQueueItem(index, queueRun.runId),
        agentId: '',
        status: item.prompt.trim() ? 'pending' : 'failed',
        message: item.prompt.trim() ? '' : 'empty prompt',
      }));
      renderQueue();
      for (let index = 0; index < queueItems.length; index += 1) {
        const item = queueItems[index];
        queueRun.activeIndex = index;
        if (queueRun.stop || item.status === 'failed') break;
        item.status = 'starting';
        item.message = 'starting clean agent context';
        renderQueue();
        const payload = await sendChat(queueCommand(item, index), 'queue');
        if (!payload?.ok) {
          item.status = 'failed';
          item.message = payload?.output || 'failed to start agent';
          renderQueue();
          break;
        }
        item.agentId = parseStartedAgentId(payload.output);
        if (!item.agentId) {
          item.status = 'failed';
          item.message = 'agent start response did not include an agent id';
          renderQueue();
          break;
        }
        item.status = 'running';
        item.message = `agent ${item.agentId}`;
        renderQueue();
        const result = await waitForQueueTask(item);
        item.status = result.ok ? 'success' : result.status === 'stopped' ? 'stopped' : 'failed';
        item.message = result.status === 'closed:success'
          ? 'closed successfully'
          : `${result.status}: ${result.message}`;
        renderQueue();
        if (!result.ok) break;
      }
      queueRun.running = false;
      queueRun.stop = false;
      queueRun.activeIndex = -1;
      renderQueue();
      await loadSnapshot();
    }

    function stopQueue() {
      queueRun.stop = true;
      if (queueRun.running && queueRun.activeIndex >= 0) {
        queueItems[queueRun.activeIndex].message = 'stop requested; current task is not killed';
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
    }

    document.getElementById('send-chat').addEventListener('click', () => sendChat(chatInput.value));
    document.getElementById('add-queue-task').addEventListener('click', addQueueTask);
    document.getElementById('run-queue').addEventListener('click', runQueue);
    document.getElementById('stop-queue').addEventListener('click', stopQueue);
    queueInput.addEventListener('keydown', (event) => {
      if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') addQueueTask();
    });
    queueInput.addEventListener('input', renderQueue);
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
