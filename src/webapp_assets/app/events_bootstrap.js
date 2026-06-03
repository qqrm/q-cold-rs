    document.getElementById('close-transcript').addEventListener('click', closeTaskTranscript);
    transcriptSend.addEventListener('click', sendTranscriptMessage);
    transcriptInput.addEventListener('keydown', (event) => {
      if (handleTerminalSlashMenuKey(transcriptInput, event)) return;
      if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') sendTranscriptMessage();
    });
    setupTerminalSlashMenu(transcriptInput, transcriptCompose);
    transcriptModal.addEventListener('click', (event) => {
      if (event.target === transcriptModal) closeTaskTranscript();
    });
    document.getElementById('add-queue-task').addEventListener('click', addQueueTask);
    document.getElementById('add-queue-wave').addEventListener('click', createQueueWave);
    document.getElementById('clear-queue').addEventListener('click', clearQueue);
    createQueueTabButton.addEventListener('click', createQueueTab);
    document.getElementById('run-queue').addEventListener('click', runQueue);
    document.getElementById('stop-queue').addEventListener('click', stopQueue);
    document.getElementById('refresh-agent-limits').addEventListener('click', () => loadAgentLimits(true));
    queueGraphModeInput.addEventListener('change', () => {
      queueGraphMode = queueGraphModeInput.checked;
      localStorage.setItem(queueGraphModeStorageKey, queueGraphMode ? '1' : '0');
      if (!queueHasBackendRun()) saveQueueStorage();
      renderQueue();
    });
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
    startStateWatcher();
    connectEvents();
    window.addEventListener('hashchange', () => setActiveView(preferredView()));
    document.addEventListener('visibilitychange', () => {
      if (!document.hidden) {
        loadSnapshot();
        connectEvents();
      }
    });
    window.addEventListener('focus', loadSnapshot);
    window.addEventListener('online', () => {
      loadSnapshot();
      connectEvents();
    });
