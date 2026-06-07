const QcoldApi = (() => {
  const writeTokenStorageKey = 'qcold-webapp-write-token-v1';

  function writeToken() {
    try {
      return sessionStorage.getItem(writeTokenStorageKey) || '';
    } catch (_err) {
      return '';
    }
  }

  function setWriteToken(token) {
    try {
      const value = String(token || '').trim();
      if (value) {
        sessionStorage.setItem(writeTokenStorageKey, value);
      } else {
        sessionStorage.removeItem(writeTokenStorageKey);
      }
    } catch (_err) {
      // A blocked sessionStorage write should not break read-only dashboard usage.
    }
  }

  function clearWriteToken() {
    setWriteToken('');
  }

  function writeTokenConfigured() {
    return Boolean(writeToken());
  }

  function errorMessage(response, payload, fallback = 'request failed') {
    const message = payload?.output || payload?.error || payload?.message || '';
    if (message) return String(message);
    if (!response) return fallback;
    const label = response.statusText ? `${response.status} ${response.statusText}` : response.status;
    return response.ok ? fallback : `HTTP ${label}`;
  }

  function tokenRequiredMessage(payload) {
    const message = String(payload?.output || payload?.error || payload?.message || '');
    return /write[- ]token|X-QCOLD-Write-Token|GUI write token/i.test(message);
  }

  async function parseJsonResponse(response) {
    const text = await response.text();
    if (!text.trim()) return {};
    try {
      return JSON.parse(text);
    } catch (_err) {
      return {
        ok: false,
        output: response.ok ? 'invalid JSON response' : `HTTP ${response.status}`,
      };
    }
  }

  async function request(path, options = {}) {
    const method = options.method || 'GET';
    const headers = { ...(options.headers || {}) };
    const requestOptions = { method, headers };
    if (options.noStore) requestOptions.cache = 'no-store';
    if (Object.prototype.hasOwnProperty.call(options, 'json')) {
      headers['content-type'] = 'application/json';
      requestOptions.body = JSON.stringify(options.json);
    } else if (Object.prototype.hasOwnProperty.call(options, 'body')) {
      requestOptions.body = options.body;
    }
    if (options.mutating) {
      const token = writeToken();
      if (token) headers['x-qcold-write-token'] = token;
    }
    return fetch(path, requestOptions);
  }

  async function jsonRequest(path, options = {}) {
    try {
      const response = await request(path, options);
      const payload = await parseJsonResponse(response);
      if (!response.ok && payload.ok !== false) payload.ok = false;
      if (!response.ok && !payload.output) payload.output = errorMessage(response, payload);
      if (options.mutating && !writeTokenConfigured() && tokenRequiredMessage(payload)) {
        payload.output = 'Dashboard write token required; enter it in the header.';
      }
      return payload;
    } catch (err) {
      return { ok: false, output: String(err) };
    }
  }

  async function readJson(path) {
    const payload = await jsonRequest(path, { noStore: true });
    if (payload.ok === false) throw new Error(payload.output || 'request failed');
    return payload;
  }

  function mutation(path, json) {
    return jsonRequest(path, {
      method: 'POST',
      mutating: true,
      json,
    });
  }

  return {
    writeTokenConfigured,
    setWriteToken,
    clearWriteToken,
    getState: () => readJson('/api/state'),
    getAgentLimits: (refresh) => readJson(`/api/agent-limits${refresh ? '?refresh=true' : ''}`),
    getTaskTranscript: (taskId) => jsonRequest(
      `/api/task-transcript?id=${encodeURIComponent(taskId)}`,
      { noStore: true },
    ),
    sendTaskChat: (taskId, target, text) => mutation('/api/task-chat/send', { task_id: taskId, target, text }),
    ensureTaskChatTarget: (taskId) => mutation('/api/task-chat/target', { task_id: taskId }),
    sendTerminal: (target, text, options = {}) => mutation('/api/terminal/send', { target, text, ...options }),
    saveTerminalMetadata: (target, name, scope) => mutation('/api/terminal/metadata', { target, name, scope }),
    runQueue: (payload) => mutation('/api/queue/run', payload),
    appendQueue: (runId, items) => mutation('/api/queue/append', { run_id: runId, items }),
    updateQueue: (runId, items) => mutation('/api/queue/update', { run_id: runId, items }),
    removeQueueItem: (payload) => mutation('/api/queue/remove', payload),
    clearQueue: (runId) => mutation('/api/queue/clear', { run_id: runId }),
    stopQueue: (runId) => mutation('/api/queue/stop', { run_id: runId }),
    continueQueue: (runId) => mutation('/api/queue/continue', { run_id: runId }),
    createQueueTab: (label) => mutation('/api/queue/tab/create', { label }),
    deleteQueueTab: (tabId) => mutation('/api/queue/tab/delete', { tab_id: tabId }),
  };
})();
