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
      const state = {
        fg: null,
        bg: null,
        bold: false,
        dim: false,
        italic: false,
        underline: false,
        inverse: false,
      };
      let buffer = '';
      const flush = () => {
        if (!buffer) return;
        const styled = state.fg
          || state.bg
          || state.bold
          || state.dim
          || state.italic
          || state.underline
          || state.inverse;
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
        ? `<strong>${formatNumber(tokenUsage.displayed_total_tokens)}</strong><span>tokens</span>` +
          `<small>${formatNumber(tokenUsage.output_tokens)} out / ` +
          `${formatNumber(tokenUsage.reasoning_output_tokens)} reasoning</small>`
        : '<strong>-</strong><span>tokens</span><small>no telemetry</small>';
      const efficiency = task.token_efficiency;
      if (efficiency) {
        const detail = document.createElement('small');
        detail.textContent = `${formatNumber(efficiency.tool_output_original_tokens)} tool output / `
          + `${formatNumber(efficiency.large_tool_output_calls)} large`;
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
      const hostAgentCount = hostRecords.filter((agent) => agent.kind !== 'web-daemon').length;
      const daemonCount = hostRecords.length - hostAgentCount;
      document.getElementById('agent-count').textContent = `${data.runningCount} running`;
      document.getElementById('host-agent-count').textContent = daemonCount
        ? `${hostAgentCount} host / ${daemonCount} daemon`
        : `${hostAgentCount} host`;
      document.getElementById('nav-agents').textContent = String(
        Math.max(data.runningCount, hostAgentCount, available.count || 0),
      );
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
      availableAgentList.replaceChildren(
        ...records.map((agent) => availableAgentCard(agent, limits.get(agent.command))),
      );
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
      const pageScroll = captureTerminalPageScroll();
      document.getElementById('terminal-count').textContent = `${terminals.count} attachable`;
      document.getElementById('nav-terminals').textContent = String(terminals.count);
      if (!terminals.records.length) {
        terminalList.innerHTML =
          `<div class="empty">${model.hostAgents.count} host agents detected, ` +
          'but no attachable terminal sessions. Start new agents through Q-COLD so they run in ' +
          'managed terminal sessions.</div>';
        terminalOutputCache.clear();
        restoreTerminalPageScroll(pageScroll);
        return;
      }
      terminalList.querySelectorAll('.empty').forEach((node) => node.remove());
      const targets = new Set(terminals.records.map((terminal) => terminal.target));
      Array.from(terminalList.querySelectorAll('.terminal-card')).forEach((node) => {
        if (!targets.has(node.dataset.target)) {
          terminalOutputCache.delete(node.dataset.target);
          node.remove();
        }
      });
      terminals.records.forEach((terminal, index) => {
        let node = terminalList.querySelector(`.terminal-card[data-target="${cssEscape(terminal.target)}"]`);
        if (!node) {
          node = createTerminalCard(terminal);
        }
        const reference = terminalList.children[index] || null;
        if (reference !== node) terminalList.insertBefore(node, reference);
        updateTerminalCard(node, terminal);
      });
      restoreTerminalPageScroll(pageScroll);
    }

    function cssEscape(value) {
      if (window.CSS && CSS.escape) return CSS.escape(value);
      return String(value).replace(/["\\]/g, '\\$&');
    }

    function captureTerminalPageScroll() {
      return document.getElementById('view-terminals').classList.contains('active')
        ? { left: window.scrollX, top: window.scrollY }
        : null;
    }

    function restoreTerminalPageScroll(position) {
      if (!position) return;
      window.scrollTo(position.left, position.top);
      window.requestAnimationFrame(() => window.scrollTo(position.left, position.top));
    }

    function createTerminalCard(terminal) {
      const node = document.createElement('article');
      node.className = 'terminal-card';
      node.dataset.target = terminal.target;
      node.dataset.agentId = terminal.agent_id || '';
      const head = document.createElement('div');
      head.className = 'terminal-head';
      head.innerHTML =
        '<div data-role="title"></div><span data-role="activity"></span><span data-role="kind"></span>' +
        '<span data-role="cwd"></span>';
      const output = document.createElement('pre');
      output.className = 'terminal-output';
      output.tabIndex = 0;
      output.addEventListener('keydown', (event) => handleTerminalKeyboard(event, terminal.target));
      const compose = terminalComposer(terminal);
      node.append(head, output, compose);
      return node;
    }

    function terminalActivityBadge(node, terminal, nextOutput) {
      const previousOutput = terminalOutputCache.get(terminal.target);
      const hasPreviousSnapshot = terminalOutputCache.has(terminal.target);
      const hasOutput = nextOutput.length > 0;
      const changed = hasPreviousSnapshot && previousOutput !== nextOutput;
      if (changed) node.dataset.lastOutputChangeAt = String(Date.now());

      const badgeNode = document.createElement('span');
      let tone = 'ready';
      let text = 'live idle - no recent output';
      if (!hasOutput) {
        tone = 'warn';
        text = 'waiting for first output';
      } else if (changed) {
        tone = 'open';
        text = 'receiving output';
      }
      badgeNode.className = `badge ${tone} terminal-activity`;
      badgeNode.textContent = text;
      return badgeNode;
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
      head.querySelector('[data-role="activity"]').replaceChildren(terminalActivityBadge(node, terminal, nextOutput));
      if (terminalOutputCache.get(terminal.target) !== nextOutput) {
        const shouldFollowTail = !terminalOutputCache.has(terminal.target) || isTerminalAtTail(output);
        const previousScrollTop = output.scrollTop;
        renderAnsi(output, nextOutput);
