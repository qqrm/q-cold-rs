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
      queueWaves = normalizeQueueWaves(queueWaves, queueItems, { pruneBackendEmpty: true });
      if (queueGraphLayoutEditable()) syncQueueWaveDependencies();
      const board = document.createElement('div');
      board.className = 'queue-graph-board';
      const levels = queueGraphLevels();
      const toolbar = document.createElement('div');
      toolbar.className = 'queue-graph-toolbar';
      const hint = document.createElement('span');
      const lockedHint = 'Only pending tasks can be edited in an active run.';
      hint.textContent = queueLayoutLocked()
        ? lockedHint
        : queueHasBackendRun()
          ? 'Unclaimed waves can be edited while the queue runs.'
          : 'Waves run top to bottom. Drag a wave header to reorder waves.';
      toolbar.append(hint);
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
        if (queueLayoutLocked()) return;
        event.preventDefault();
        if (queueHasBackendRun() && !queueWaveEditable(level)) return;
        const sourceWaveId = event.dataTransfer.getData('text/qcold-queue-wave');
        if (sourceWaveId) {
          moveQueueWaveTo(sourceWaveId, level.wave.id);
          return;
        }
        const sourceId = event.dataTransfer.getData('text/qcold-queue-item');
        moveQueueItemToWave(sourceId, level.wave.id);
      });
      const head = document.createElement('div');
      head.className = 'queue-graph-wave-head';
      head.draggable = queueWaveEditable(level);
      head.addEventListener('dragstart', (event) => {
        if (!queueWaveEditable(level)) return;
        event.dataTransfer.effectAllowed = 'move';
        event.dataTransfer.setData('text/qcold-queue-wave', level.wave.id);
      });
      const headTitle = document.createElement('div');
      headTitle.className = 'queue-graph-wave-title';
      const heading = document.createElement('h3');
      heading.textContent = `Wave ${index + 1}`;
      const meta = document.createElement('span');
      meta.textContent = `${level.items.length} task${level.items.length === 1 ? '' : 's'}`;
      headTitle.append(heading, meta);
      const remove = queueActionButton('×', () => removeQueueWave(level.wave.id), 'Remove wave');
      remove.classList.add('danger', 'icon-remove', 'queue-remove-corner');
      remove.hidden = !queueGraphLayoutEditable();
      remove.disabled = !queueGraphLayoutEditable() || queueWaves.length <= 1 || level.items.length > 0;
      head.append(headTitle);
      const lane = document.createElement('div');
      lane.className = 'queue-graph-wave-lane';
      if (!level.items.length) {
        const empty = document.createElement('p');
        empty.className = 'queue-graph-empty-wave';
        empty.textContent = 'Drop a task here.';
        lane.appendChild(empty);
      }
      level.items.forEach((item) => lane.appendChild(queueGraphCard(item)));
      column.append(remove, head, lane);
      return column;
    }

    function queueGraphLevels() {
      return queueWaves.map((wave) => ({
        wave,
        items: queueItems.filter((item) => item.waveId === wave.id),
      }));
    }

    function normalizeQueueWaves(waves, items, options = {}) {
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
      return options.pruneBackendEmpty ? pruneEmptyBackendQueueWaves(normalized, items) : normalized;
    }

    function pruneEmptyBackendQueueWaves(waves, items) {
      if (!queueHasBackendRun() || !queueGraphMode || waves.length <= 1) return waves;
      const wavesWithItems = new Set(items.map((item) => item.waveId).filter(Boolean));
      const pruned = waves.filter((wave, index) => {
        return wavesWithItems.has(wave.id) || index === waves.length - 1;
      });
      return pruned.length ? pruned : [lastQueueWave(waves)];
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
      if (!queueGraphAppendable()) return;
      queueWaves.push({ id: newQueueWaveId() });
      if (!queueHasBackendRun()) saveQueueStorage();
      renderQueue();
    }

    function removeQueueWave(waveId) {
      if (!queueGraphLayoutEditable() || queueWaves.length <= 1) return;
      if (queueItems.some((item) => item.waveId === waveId)) return;
      queueWaves = queueWaves.filter((wave) => wave.id !== waveId);
      saveQueueStorage();
      renderQueue();
    }

    function moveQueueWaveTo(sourceWaveId, targetWaveId) {
      if (!sourceWaveId || !targetWaveId || sourceWaveId === targetWaveId || queueLayoutLocked()) {
        return;
      }
      const sourceIndex = queueWaves.findIndex((candidate) => candidate.id === sourceWaveId);
      const targetIndex = queueWaves.findIndex((candidate) => candidate.id === targetWaveId);
      if (sourceIndex < 0 || targetIndex < 0) return;
      const sourceLevel = queueGraphLevels().find((level) => level.wave.id === sourceWaveId);
      if (!sourceLevel || !queueWaveEditable(sourceLevel)) return;
      const [wave] = queueWaves.splice(sourceIndex, 1);
      queueWaves.splice(targetIndex, 0, wave);
      syncQueueWaveDependencies();
      saveQueueStorage();
      renderQueue();
      persistQueuePlan();
    }

    function moveQueueItemToWave(itemId, waveId) {
      if (!itemId || !waveId || queueLayoutLocked()) return;
      const item = queueItems.find((candidate) => candidate.id === itemId);
      if (!item || item.waveId === waveId || !queueItemEditable(item)) return;
      const targetLevel = queueGraphLevels().find((level) => level.wave.id === waveId);
      if (queueHasBackendRun() && (!targetLevel || !queueWaveEditable(targetLevel))) return;
      item.waveId = waveId;
      syncQueueWaveDependencies();
      saveQueueStorage();
      renderQueue();
      persistQueuePlan();
    }

    function syncQueueWaveDependencies() {
      const waveItems = queueGraphLevels().map((level) => level.items);
      let previousGates = [];
      for (const items of waveItems) {
        for (const item of items) {
          if (!queueHasBackendRun() || queueItemEditable(item)) {
            item.dependsOn = previousGates.map((dependency) => dependency.id);
          }
        }
        previousGates = items.filter((item) => item.gatesNext !== false);
      }
    }

    function queueLayoutLocked() {
      return queueHasBackendRun() ? !queueBackendRunEditable() : false;
    }

    function queueGraphLayoutEditable() {
      return !queueHasBackendRun() || (queueGraphMode && queueBackendRunEditable());
    }

    function queueBackendRunEditable() {
      if (!queueHasBackendRun()) return false;
      return ['running', 'waiting', 'starting', 'stopped'].includes(queueRun.status);
    }

    function queueGraphAppendable() {
      return queueGraphMode && (!queueHasBackendRun() || queueBackendRunAppendable());
    }

    function queueBackendRunAppendable() {
      if (!queueHasBackendRun()) return false;
      return ['running', 'waiting', 'starting', 'stopped'].includes(queueRun.status);
    }

    function queueHasBackendRun() {
      return Boolean(queueRun.runId || queueItems.some((item) => item.runId));
    }

    function queueWaveEditable(level) {
      return queueGraphLayoutEditable()
        && (!queueHasBackendRun() || level.items.every((item) => queueItemEditable(item)));
    }

    function queueHasDraftGraph() {
      return queueGraphMode && queueWaves.length > 1;
    }

    function queueCanClear() {
      return Boolean(queueItems.length || queueHasBackendRun() || queueHasDraftGraph());
    }

    function queueShouldRenderEmptyGraph() {
      return Boolean(queueGraphMode && (queueHasBackendRun() || queueHasDraftGraph()));
    }

    function queueGraphCard(item) {
      const index = queueItems.findIndex((candidate) => candidate.id === item.id);
      const view = queueItemView(item);
      const card = document.createElement('article');
      card.className = `queue-graph-card ${view.status}`;
      card.draggable = queueItemEditable(item);
      card.dataset.itemId = item.id;
      card.addEventListener('dragstart', (event) => {
        if (!queueItemEditable(item)) return;
        event.stopPropagation();
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

      const head = document.createElement('div');
      head.className = 'queue-graph-card-head';
      const title = document.createElement('div');
      title.className = 'queue-graph-card-title';
      title.append(badge(queueStatusText(item)), document.createTextNode(` #${index + 1}`));
      const remove = queueRemoveButton(index);
      remove.classList.add('queue-remove-corner');
      head.append(title);
      const prompt = document.createElement('p');
      prompt.className = 'queue-graph-prompt-preview';
      prompt.textContent = queuePromptPreview(item.prompt);
      prompt.title = 'Use Full prompt to inspect the complete text';
      const direction = document.createElement('p');
      direction.className = 'queue-graph-card-hint';
      direction.textContent = !queueItemEditable(item)
        ? 'Task is owned by the backend run.'
        : 'Drag into a wave to move this task.';
      const gate = queueGateToggle(item);
      const controls = queueGraphCardControls(index);
      const fullPrompt = queueActionButton(
        'Full prompt',
        () => openQueuePromptModal(item, index),
        'Show full prompt',
      );
      fullPrompt.classList.add('queue-graph-prompt-action');
      controls.prepend(fullPrompt);
      card.append(remove, head, prompt, direction, gate, controls);
      return card;
    }

    function queueGateToggle(item) {
      const label = document.createElement('label');
      label.className = 'queue-graph-gate-toggle';
      label.title = 'When enabled, later waves wait for this task to finish successfully.';
      const input = document.createElement('input');
      input.type = 'checkbox';
      input.checked = item.gatesNext !== false;
      input.disabled = !queueItemEditable(item);
      input.setAttribute('aria-label', 'Blocks next wave');
      input.addEventListener('change', () => {
        item.gatesNext = input.checked;
        syncQueueWaveDependencies();
        saveQueueStorage();
        renderQueue();
        persistQueuePlan();
      });
      label.append(input, document.createTextNode('Blocks next wave'));
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
      const editor = modal.querySelector('[data-queue-prompt-editor]');
      const save = modal.querySelector('[data-queue-prompt-save]');
      const editable = queueItemEditable(item);
      title.textContent = `Task #${index + 1} full prompt`;
      prompt.textContent = item.prompt || '(empty prompt)';
      editor.value = item.prompt || '';
      prompt.hidden = editable;
      editor.hidden = !editable;
      save.hidden = !editable;
      save.onclick = () => saveQueuePromptEdit(item.id, editor.value);
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
            <button class="secondary" type="button" data-queue-prompt-save>Save</button>
            <button class="secondary" type="button" data-queue-prompt-close>Close</button>
          </div>
          <textarea class="queue-prompt-editor" data-queue-prompt-editor hidden></textarea>
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
      if (queueLayoutLocked()) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = 'link';
    }

    async function saveQueuePromptEdit(itemId, prompt) {
      const item = queueItems.find((candidate) => candidate.id === itemId);
      const text = String(prompt || '').trim();
      if (!item || !text || !queueItemEditable(item)) return;
      item.prompt = text;
      saveQueueStorage();
      renderQueue();
      const modal = document.getElementById('queue-prompt-modal');
      if (modal) modal.hidden = true;
      await persistQueuePlan();
    }

    function queueRunningText() {
      const activePosition = Number(queueRun.activeIndex);
      if (queueGraphMode || queueRun.status === 'graph') {
        const active = queueItems.filter((item) => {
          return ['starting', 'running', 'waiting'].includes(queueItemView(item).status);
        }).length;
        const verb = queueRun.status === 'starting' ? 'starting' : 'running';
        return `${verb} ${active}/${queueItems.length}`;
      }
      const visibleIndex = queueItems.findIndex((item) => Number(item.position) === activePosition);
      const ordinal = visibleIndex >= 0
        ? visibleIndex + 1
        : Math.min(Math.max(activePosition + 1, 0), queueItems.length);
      const verb = queueRun.status === 'starting' ? 'starting' : 'running';
      return `${verb} ${ordinal}/${queueItems.length}`;
    }

    async function addQueueTask() {
      const prompt = queueInput.value.trim();
      if (!prompt) return;
      if (queueBackendRunAppendable()) {
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
      queueWaves = normalizeQueueWaves(queueWaves, queueItems);
      const item = {
        ...defaultQueueItem(),
        prompt,
        waveId: queueGraphMode ? lastQueueWave(queueWaves).id : '',
      };
      const dependsOn = queueGraphMode ? queueDependenciesForWave(item.waveId) : [];
      try {
        const response = await fetch('/api/queue/append', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            run_id: runId,
            items: [{ id: item.id, prompt, depends_on: dependsOn }],
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

    function queueDependenciesForWave(waveId) {
      let previousGates = [];
      for (const level of queueGraphLevels()) {
        if (level.wave.id === waveId) {
          return previousGates.map((dependency) => dependency.id);
        }
        previousGates = level.items.filter((item) => item.gatesNext !== false);
      }
      return previousGates.map((dependency) => dependency.id);
    }

    function queueItemControls(index, options = {}) {
      const controls = document.createElement('div');
      controls.className = 'queue-step-actions';
      const up = queueActionButton('↑', () => moveQueueItem(index, -1), 'Move task up');
      up.disabled = !queueItemCanMove(index, -1);
      const down = queueActionButton('↓', () => moveQueueItem(index, 1), 'Move task down');
      down.disabled = !queueItemCanMove(index, 1);
      const open = queueActionButton('↗', () => openQueueItemContext(index), 'Open task chat or transcript');
      open.disabled = !queueItemContextTarget(queueItems[index]);
      const copy = queueActionButton('', () => copyQueuePrompt(index), 'Copy prompt');
      copy.classList.add('icon-copy');
      controls.append(up, down, open, copy);
      if (options.includeRemove !== false) controls.append(queueRemoveButton(index));
      return controls;
    }

    function queueRemoveButton(index) {
      const remove = queueActionButton('×', () => removeQueueItem(index), 'Remove task');
      remove.classList.add('danger', 'icon-remove');
      if (queueItemRemoving(queueItems[index])) {
        remove.textContent = '…';
      }
      remove.disabled = queueItemRemoving(queueItems[index]) || !queueItemRemovable(queueItems[index]);
      return remove;
    }

    function queueGraphCardControls(index) {
      const controls = document.createElement('div');
      controls.className = 'queue-step-actions queue-graph-card-actions';
      const open = queueActionButton('↗', () => openQueueItemContext(index), 'Open task chat or transcript');
      open.disabled = !queueItemContextTarget(queueItems[index]);
      const copy = queueActionButton('', () => copyQueuePrompt(index), 'Copy prompt');
      copy.classList.add('icon-copy');
      controls.append(open, copy);
      return controls;
    }

    function queueActionButton(label, action, title = label) {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'secondary compact queue-icon-button';
      button.textContent = label;
      button.title = title;
      button.setAttribute('aria-label', title);
      button.addEventListener('click', action);
      return button;
    }

    function moveQueueItem(index, delta) {
      const next = index + delta;
      if (!queueItemCanMove(index, delta)) return;
      const [item] = queueItems.splice(index, 1);
      queueItems.splice(next, 0, item);
      saveQueueStorage();
      renderQueue();
      persistQueuePlan();
    }

    function queueItemCanMove(index, delta) {
      const next = index + delta;
      if (next < 0 || next >= queueItems.length || queueLayoutLocked()) return false;
      const item = queueItems[index];
      const target = queueItems[next];
      if (!queueItemEditable(item)) return false;
      if (queueHasBackendRun() && target && !queueItemEditable(target)) return false;
      return true;
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
        && queueBackendRunEditable()
        && Number(item.position) > activePosition;
    }

    function queueItemEditable(item) {
      if (!item) return false;
      if (!queueHasBackendRun()) return true;
      const view = queueItemView(item);
      const activePosition = Number(queueRun.activeIndex);
      return Boolean(item.runId)
        && ['pending', 'waiting'].includes(view.status)
        && !item.agentId
        && Number(item.position) > activePosition;
    }

    async function persistQueuePlan() {
      if (!queueHasBackendRun()) return;
      const runId = queueRun.runId || queueItems.find((item) => item.runId)?.runId || '';
      if (!runId) return;
      const items = queueItems
        .map((item, index) => ({ item, index }))
        .filter(({ item }) => queueItemEditable(item));
      if (!items.length) return;
      try {
        const response = await fetch('/api/queue/update', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            run_id: runId,
            items: items.map(({ item, index }) => ({
              id: item.id,
              prompt: item.prompt,
              position: index,
              depends_on: queueGraphMode ? (item.dependsOn || []) : [],
              repo_root: item.repoRoot,
              repo_name: item.repoName,
              agent_command: item.agentCommand,
            })),
          }),
        });
        const payload = await response.json().catch(() => ({}));
        if (!response.ok || payload.ok === false) {
          appendLocalMessage('error', payload.output || 'failed to update queue plan');
          await loadSnapshot();
          return;
        }
        await loadSnapshot();
      } catch (err) {
        appendLocalMessage('error', String(err));
        await loadSnapshot();
      }
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
      const runId = queueRun.runId || queueItems.find((item) => item.runId)?.runId || '';
      if (!queueCanClear() && !runId) return;
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
      if (runId) await loadSnapshot();
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
      if (target?.kind === 'task-modal') {
        openTaskTranscript(target.taskId, { terminal: target.terminal });
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
      const taskId = task?.id || (item.slug ? `task/${item.slug}` : '');
      if (task?.id && task.session_path) {
        return { kind: 'transcript', task, terminal: terminalForQueueItem(item, task) };
      }
      const terminal = terminalForQueueItem(item, task);
      if (task?.id && terminal) {
        return { kind: 'terminal-chat', task, terminal };
      }
      if (taskId) {
        return { kind: 'task-modal', taskId, task, terminal };
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
        scrollTranscriptToEnd();
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
        scrollTranscriptToEnd();
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
      const block = document.createElement('section');
      block.className = 'transcript-terminal-block';
      const head = document.createElement('div');
      head.className = 'transcript-terminal-head';
      const title = document.createElement('span');
      title.textContent = 'Live terminal';
      const meta = document.createElement('small');
      meta.textContent = terminal.label || terminal.agent_id || terminal.target || '';
      head.append(title, meta);
      const output = document.createElement('div');
      output.className = 'terminal-output transcript-terminal-output';
      output.tabIndex = 0;
      output.addEventListener('keydown', (event) => handleTerminalKeyboard(event, terminal.target));
      renderAnsi(output, terminal.output);
      block.append(head, output);
      return block;
    }

    function scrollTranscriptToEnd() {
      const scroll = () => {
        const liveOutput = transcriptLog.querySelector('.transcript-terminal-output');
        if (liveOutput) liveOutput.scrollTop = liveOutput.scrollHeight;
        transcriptLog.scrollTop = transcriptLog.scrollHeight;
      };
      scroll();
      window.requestAnimationFrame(scroll);
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
