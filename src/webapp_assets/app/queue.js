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
      stopButton.classList.toggle('visible', queueRun.running || queueRun.stopped);
    }

    function renderQueueGraph() {
      queueWaves = normalizeQueueWaves(queueWaves, queueItems);
      syncQueueWaveDependencies();
      const board = document.createElement('div');
      board.className = 'queue-graph-board';
      const levels = queueGraphLevels();
      const toolbar = document.createElement('div');
      toolbar.className = 'queue-graph-toolbar';
      const hint = document.createElement('span');
      hint.textContent = 'Waves run top to bottom. Tasks inside one wave run in parallel.';
      const addWave = queueActionButton('Add wave', createQueueWave, 'Add wave');
      addWave.classList.add('queue-graph-add-wave');
      addWave.disabled = queueRun.running;
      toolbar.append(hint, addWave);
      board.appendChild(toolbar);

      levels.forEach((level, index) => {
        board.appendChild(queueGraphWave(level, index));
      });
      queueStatus.replaceChildren(board);
    }

    function queueGraphWave(level, index) {
      const column = document.createElement('section');
      column.className = 'queue-graph-wave';
      column.dataset.waveId = level.wave.id;
      column.addEventListener('dragover', allowQueueGraphDrop);
      column.addEventListener('drop', (event) => {
        event.preventDefault();
        const sourceId = event.dataTransfer.getData('text/qcold-queue-item');
        moveQueueItemToWave(sourceId, level.wave.id);
      });
      const head = document.createElement('div');
      head.className = 'queue-graph-wave-head';
      const heading = document.createElement('h3');
      heading.textContent = `Wave ${index + 1}`;
      const meta = document.createElement('span');
      meta.textContent = `${level.items.length} task${level.items.length === 1 ? '' : 's'}`;
      const up = queueActionButton('↑', () => moveQueueWave(level.wave.id, -1), 'Move wave up');
      up.disabled = queueRun.running || index === 0;
      const down = queueActionButton('↓', () => moveQueueWave(level.wave.id, 1), 'Move wave down');
      down.disabled = queueRun.running || index === queueWaves.length - 1;
      const remove = queueActionButton('×', () => removeQueueWave(level.wave.id), 'Remove wave');
      remove.classList.add('danger', 'icon-remove');
      remove.disabled = queueRun.running || queueWaves.length <= 1 || level.items.length > 0;
      head.append(heading, meta, up, down, remove);
      const lane = document.createElement('div');
      lane.className = 'queue-graph-wave-lane';
      if (!level.items.length) {
        const empty = document.createElement('p');
        empty.className = 'queue-graph-empty-wave';
        empty.textContent = 'Drop a task here.';
        lane.appendChild(empty);
      }
      level.items.forEach((item) => lane.appendChild(queueGraphCard(item)));
      column.append(head, lane);
      return column;
    }

    function queueGraphLevels() {
      return queueWaves.map((wave) => ({
        wave,
        items: queueItems.filter((item) => item.waveId === wave.id),
      }));
    }

    function normalizeQueueWaves(waves, items) {
      const normalized = (Array.isArray(waves) ? waves : [])
        .map((wave) => (typeof wave === 'string' ? { id: wave } : wave))
        .filter((wave) => wave?.id)
        .map((wave) => ({ id: wave.id }));
      if (!normalized.length) normalized.push({ id: newQueueWaveId() });
      let known = new Set(normalized.map((wave) => wave.id));
      const missing = items.filter((item) => !item.waveId || !known.has(item.waveId));
      if (missing.some((item) => item.dependsOn?.length)) {
        assignQueueWavesFromDependencies(normalized, items);
        known = new Set(normalized.map((wave) => wave.id));
      }
      const lastWave = lastQueueWave(normalized);
      for (const item of items) {
        if (!item.waveId || !known.has(item.waveId)) item.waveId = lastWave.id;
      }
      return normalized;
    }

    function lastQueueWave(waves = queueWaves) {
      return waves[waves.length - 1];
    }

    function assignQueueWavesFromDependencies(waves, items) {
      const byId = new Map(items.map((item) => [item.id, item]));
      const memo = new Map();
      const depth = (item, stack = new Set()) => {
        if (!item) return 0;
        if (memo.has(item.id)) return memo.get(item.id);
        if (stack.has(item.id)) return 0;
        stack.add(item.id);
        const value = Math.max(0, ...(item.dependsOn || [])
          .map((dependency) => depth(byId.get(dependency), stack) + 1));
        stack.delete(item.id);
        memo.set(item.id, value);
        return value;
      };
      items.forEach((item) => {
        const level = depth(item);
        while (!waves[level]) waves.push({ id: newQueueWaveId() });
        item.waveId = waves[level].id;
      });
    }

    function newQueueWaveId() {
      return `wave-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    }

    function createQueueWave() {
      if (queueRun.running) return;
      queueWaves.push({ id: newQueueWaveId() });
      saveQueueStorage();
      renderQueue();
    }

    function removeQueueWave(waveId) {
      if (queueRun.running || queueWaves.length <= 1) return;
      if (queueItems.some((item) => item.waveId === waveId)) return;
      queueWaves = queueWaves.filter((wave) => wave.id !== waveId);
      saveQueueStorage();
      renderQueue();
    }

    function moveQueueWave(waveId, delta) {
      if (queueRun.running) return;
      const index = queueWaves.findIndex((candidate) => candidate.id === waveId);
      const next = index + delta;
      if (index < 0 || next < 0 || next >= queueWaves.length) return;
      const [wave] = queueWaves.splice(index, 1);
      queueWaves.splice(next, 0, wave);
      syncQueueWaveDependencies();
      saveQueueStorage();
      renderQueue();
    }

    function moveQueueItemToWave(itemId, waveId) {
      if (!itemId || !waveId || queueRun.running) return;
      const item = queueItems.find((candidate) => candidate.id === itemId);
      if (!item || item.waveId === waveId) return;
      item.waveId = waveId;
      syncQueueWaveDependencies();
      saveQueueStorage();
      renderQueue();
    }

    function syncQueueWaveDependencies() {
      const waveItems = queueGraphLevels().map((level) => level.items);
      let previousGates = [];
      for (const items of waveItems) {
        for (const item of items) item.dependsOn = previousGates.map((dependency) => dependency.id);
        previousGates = items.filter((item) => item.gatesNext !== false);
      }
    }

    function queueGraphCard(item) {
      const index = queueItems.findIndex((candidate) => candidate.id === item.id);
      const view = queueItemView(item);
      const card = document.createElement('article');
      card.className = `queue-graph-card ${view.status}`;
      card.draggable = !queueRun.running;
      card.dataset.itemId = item.id;
      card.addEventListener('dragstart', (event) => {
        event.dataTransfer.effectAllowed = 'linkMove';
        event.dataTransfer.setData('text/qcold-queue-item', item.id);
      });
      card.addEventListener('dragover', allowQueueGraphDrop);
      card.addEventListener('drop', (event) => {
        event.preventDefault();
        event.stopPropagation();
        const sourceId = event.dataTransfer.getData('text/qcold-queue-item');
        moveQueueItemToWave(sourceId, item.waveId);
      });

      const title = document.createElement('div');
      title.className = 'queue-graph-card-title';
      title.append(badge(queueStatusText(item)), document.createTextNode(` #${index + 1}`));
      const prompt = document.createElement('p');
      prompt.className = 'queue-graph-prompt-preview';
      prompt.textContent = queuePromptPreview(item.prompt);
      prompt.title = 'Use Full prompt to inspect the complete text';
      const direction = document.createElement('p');
      direction.className = 'queue-graph-card-hint';
      direction.textContent = queueRun.running
        ? 'Dependency chips are locked for this run.'
        : 'Choose a wave or drag into one.';
      const waveSelect = queueWaveSelect(item);
      const gate = queueGateToggle(item);
      const deps = document.createElement('div');
      deps.className = 'queue-graph-deps';
      if (item.dependsOn?.length) {
        item.dependsOn.forEach((dependency) => {
          const depIndex = queueItems.findIndex((candidate) => candidate.id === dependency);
          const chip = queueActionButton(
            `waits #${depIndex + 1}`,
            () => removeQueueDependency(dependency, item.id),
            `Remove wait for #${depIndex + 1}`,
          );
          chip.classList.add('queue-dependency-chip');
          deps.appendChild(chip);
        });
      } else {
        const chip = document.createElement('span');
        chip.className = 'badge ready';
        chip.textContent = 'runs first';
        deps.appendChild(chip);
      }
      const controls = queueItemControls(index);
      const fullPrompt = queueActionButton(
        'Full prompt',
        () => openQueuePromptModal(item, index),
        'Show full prompt',
      );
      fullPrompt.classList.add('queue-graph-prompt-action');
      controls.prepend(fullPrompt);
      card.append(title, prompt, direction, waveSelect, gate, deps, controls);
      return card;
    }

    function queueWaveSelect(item) {
      const select = document.createElement('select');
      select.className = 'queue-graph-wave-select';
      select.disabled = queueRun.running;
      queueWaves.forEach((wave, index) => {
        const option = document.createElement('option');
        option.value = wave.id;
        option.textContent = `Wave ${index + 1}`;
        select.appendChild(option);
      });
      select.value = item.waveId || queueWaves[queueWaves.length - 1]?.id || '';
      select.addEventListener('change', () => moveQueueItemToWave(item.id, select.value));
      return select;
    }

    function queueGateToggle(item) {
      const label = document.createElement('label');
      label.className = 'queue-graph-gate-toggle';
      const input = document.createElement('input');
      input.type = 'checkbox';
      input.checked = item.gatesNext !== false;
      input.disabled = queueRun.running;
      input.addEventListener('change', () => {
        item.gatesNext = input.checked;
        syncQueueWaveDependencies();
        saveQueueStorage();
        renderQueue();
      });
      label.append(input, document.createTextNode('Gate next'));
      return label;
    }

    function queuePromptPreview(prompt) {
      const line = String(prompt || '')
        .split(/\r?\n/)
        .map((value) => value.trim())
        .find(Boolean) || '(empty prompt)';
      return line.length > 72 ? `${line.slice(0, 69)}...` : line;
    }

    function openQueuePromptModal(item, index) {
      const modal = queuePromptModal();
      const title = modal.querySelector('[data-queue-prompt-title]');
      const prompt = modal.querySelector('[data-queue-prompt-text]');
      title.textContent = `Task #${index + 1} full prompt`;
      prompt.textContent = item.prompt || '(empty prompt)';
      modal.hidden = false;
    }

    function queuePromptModal() {
      let modal = document.getElementById('queue-prompt-modal');
      if (modal) return modal;
      modal = document.createElement('div');
      modal.id = 'queue-prompt-modal';
      modal.className = 'transcript-modal';
      modal.hidden = true;
      modal.innerHTML = `
        <div class="transcript-dialog" role="dialog" aria-modal="true"
          aria-labelledby="queue-prompt-modal-title">
          <div class="panel-head">
            <div>
              <h2 id="queue-prompt-modal-title" data-queue-prompt-title>Full prompt</h2>
              <span class="task-path">Queue graph task prompt</span>
            </div>
            <button class="secondary" type="button" data-queue-prompt-close>Close</button>
          </div>
          <pre class="queue-prompt-full" data-queue-prompt-text></pre>
        </div>
      `;
      modal.addEventListener('click', (event) => {
        if (event.target === modal || event.target.closest('[data-queue-prompt-close]')) {
          modal.hidden = true;
        }
      });
      document.addEventListener('keydown', (event) => {
        if (event.key === 'Escape' && !modal.hidden) modal.hidden = true;
      });
      document.body.appendChild(modal);
      return modal;
    }

    function allowQueueGraphDrop(event) {
      if (queueRun.running) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = 'link';
    }

    function addQueueDependency(sourceId, targetId) {
      if (!sourceId || !targetId || sourceId === targetId || queueRun.running) return;
      const target = queueItems.find((item) => item.id === targetId);
      if (!target) return;
      target.dependsOn = Array.from(new Set([...(target.dependsOn || []), sourceId]));
      if (queueGraphHasCycle()) {
        target.dependsOn = target.dependsOn.filter((id) => id !== sourceId);
        appendLocalMessage('error', 'Dependency would create a cycle');
        return;
      }
      saveQueueStorage();
      renderQueue();
    }

    function removeQueueDependency(sourceId, targetId) {
      const target = queueItems.find((item) => item.id === targetId);
      if (!target || queueRun.running) return;
      target.dependsOn = (target.dependsOn || []).filter((id) => id !== sourceId);
      saveQueueStorage();
      renderQueue();
    }

    function queueGraphHasCycle() {
      const byId = new Map(queueItems.map((item) => [item.id, item]));
      const visiting = new Set();
      const visited = new Set();
      const visit = (id) => {
        if (visited.has(id)) return false;
        if (visiting.has(id)) return true;
        visiting.add(id);
        const item = byId.get(id);
        const cyclic = (item?.dependsOn || []).some(visit);
        visiting.delete(id);
        visited.add(id);
        return cyclic;
      };
      return queueItems.some((item) => visit(item.id));
    }

    function queueRunningText() {
      const activePosition = Number(queueRun.activeIndex);
      if (queueGraphMode || queueRun.status === 'graph') {
        const active = queueItems.filter((item) => {
          return ['starting', 'running', 'waiting'].includes(queueItemView(item).status);
        }).length;
        return `running ${active}/${queueItems.length}`;
      }
      const visibleIndex = queueItems.findIndex((item) => Number(item.position) === activePosition);
      const ordinal = visibleIndex >= 0
        ? visibleIndex + 1
        : Math.min(Math.max(activePosition + 1, 0), queueItems.length);
      return `running ${ordinal}/${queueItems.length}`;
    }

    async function addQueueTask() {
      const prompt = queueInput.value.trim();
      if (!prompt) return;
      if (queueRun.running || queueRun.stopped) {
        await appendQueueTask(prompt);
        return;
      }
      queueWaves = normalizeQueueWaves(queueWaves, queueItems);
      const waveId = queueGraphMode ? lastQueueWave(queueWaves).id : '';
      queueItems.push({ ...defaultQueueItem(), prompt, waveId });
      queueInput.value = '';
      saveQueueStorage();
      renderQueue();
      queueInput.focus();
    }

    async function appendQueueTask(prompt) {
      const runId = queueRun.runId || state?.queue?.run?.id || '';
      if (!runId) {
        appendLocalMessage('error', 'No active queue run to append to');
        return;
      }
      const item = defaultQueueItem();
      try {
        const response = await fetch('/api/queue/append', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            run_id: runId,
            items: [{ id: item.id, prompt }],
          }),
        });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) {
          appendLocalMessage('error', payload.output || 'failed to append queue item');
          return;
        }
        queueInput.value = '';
        await loadSnapshot();
        queueInput.focus();
      } catch (err) {
        appendLocalMessage('error', String(err));
      }
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
      if (queueItemRemoving(queueItems[index])) {
        remove.textContent = '…';
      }
      remove.disabled = queueItemRemoving(queueItems[index]) || !queueItemRemovable(queueItems[index]);
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
      const item = queueItems[index];
      if (!queueItemRemovable(item) || queueItemRemoving(item)) return;
      const task = taskRecordForQueueItem(item);
      if (item?.runId) {
        removeServerQueueItem(item, task, index);
        return;
      }
      removeQueueItemLocally(index, item);
    }

    function removeQueueItemLocally(index, item) {
      let currentIndex = item?.id
        ? queueItems.findIndex((candidate) => candidate.id === item.id && candidate.runId === item.runId)
        : -1;
      if (currentIndex === -1 && item?.slug) {
        currentIndex = queueItems.findIndex((candidate) => candidate.slug === item.slug);
      }
      if (currentIndex === -1 && queueItems[index] === item) {
        currentIndex = index;
      }
      if (currentIndex === -1) return;
      const [removed] = queueItems.splice(currentIndex, 1);
      if (removed?.id) {
        queueItems.forEach((candidate) => {
          candidate.dependsOn = (candidate.dependsOn || []).filter((id) => id !== removed.id);
        });
      }
      saveQueueStorage();
      renderQueue();
    }

    function queueItemTerminal(item) {
      return ['success', 'failed', 'blocked'].includes(queueItemView(item).status);
    }

    function queueItemKey(item) {
      return `${item?.runId || ''}:${item?.id || item?.slug || ''}`;
    }

    function queueItemRemoving(item) {
      return removingQueueItems.has(queueItemKey(item));
    }

    function queueItemRemovable(item) {
      if (!item) return false;
      if (!queueRun.running) return true;
      if (queueItemTerminal(item)) return true;
      const view = queueItemView(item);
      const activePosition = Number(queueRun.activeIndex);
      return Boolean(item.runId)
        && ['pending', 'waiting'].includes(view.status)
        && !item.agentId
        && Number(item.position) > activePosition;
    }

    async function removeServerQueueItem(item, task, index) {
      const key = queueItemKey(item);
      removingQueueItems.add(key);
      removeQueueItemLocally(index, item);
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
          removingQueueItems.delete(key);
          appendLocalMessage('error', payload.output || 'failed to remove queue item');
          await loadSnapshot();
          return;
        }
        await loadSnapshot();
      } catch (err) {
        removingQueueItems.delete(key);
        appendLocalMessage('error', String(err));
        await loadSnapshot();
      } finally {
        removingQueueItems.delete(key);
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
      queueWaves = [{ id: newQueueWaveId() }];
      queueRun = { running: false, stopped: false, stop: false, activeIndex: -1, runId: '', status: '' };
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
