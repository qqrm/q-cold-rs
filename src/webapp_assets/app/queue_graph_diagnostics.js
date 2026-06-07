    function queueGraphWaveIndex(item) {
      if (!queueGraphMode || !item?.waveId) return null;
      const index = queueWaves.findIndex((wave) => wave.id === item.waveId);
      return index >= 0 ? index : null;
    }

    function queueGraphPayloadFields(item, dependsOn = item?.dependsOn || []) {
      const fields = {
        depends_on: queueGraphMode ? dependsOn : [],
      };
      if (!queueGraphMode) return fields;
      if (item?.waveId) fields.wave_id = item.waveId;
      const waveIndex = queueGraphWaveIndex(item);
      if (waveIndex !== null) fields.wave_index = waveIndex;
      return fields;
    }

    function queueRunItemPayload(item, options = {}) {
      const selectedAgent = options.selectedAgent || {};
      return {
        id: item.id,
        prompt: options.prompt ?? item.prompt,
        slug: item.slug,
        repo_root: item.repoRoot,
        repo_name: item.repoName,
        agent_command: item.agentCommand || selectedAgent.command || '',
        ...queueGraphPayloadFields(item, options.dependsOn || item.dependsOn || []),
      };
    }

    function queueUpdateItemPayload(item, index) {
      return {
        id: item.id,
        prompt: item.prompt,
        position: index,
        repo_root: item.repoRoot,
        repo_name: item.repoName,
        agent_command: item.agentCommand,
        ...queueGraphPayloadFields(item),
      };
    }

    function applyQueueGraphDiagnostics(payload, options = {}) {
      const graph = QcoldApi.queueGraphDiagnostics(payload);
      if (!graph) return;
      applyQueueGraphCanonicalItems(graph);
      const diagnostics = Array.isArray(graph.diagnostics) ? graph.diagnostics : [];
      if (!diagnostics.length) return;
      const now = Math.floor(Date.now() / 1000);
      for (const diagnostic of diagnostics) {
        const severity = String(diagnostic?.severity || 'warning').toLowerCase();
        const message = compactQueueLine(
          `${severity}: ${String(diagnostic?.message || '').trim()}`,
        );
        if (!message) continue;
        for (const item of queueGraphDiagnosticItems(diagnostic)) {
          item.message = message;
          item.updatedAt = now;
          if (options.markErrors && severity === 'error') item.status = QcoldQueueItemStatus.Failed;
        }
      }
      syncQueueGatesFromDependents(queueItems);
    }

    function applyQueueGraphCanonicalItems(graph) {
      if (!graph || !Array.isArray(graph.items)) return;
      const waves = [];
      const ensureWave = (index) => {
        while (waves.length <= index) {
          waves.push(queueWaves[waves.length] || { id: newQueueWaveId() });
        }
        return waves[index];
      };
      for (const diagnosticItem of graph.items) {
        const item = queueItems.find((candidate) => candidate.id === diagnosticItem.id);
        if (!item) continue;
        if (Array.isArray(diagnosticItem.depends_on)) {
          item.dependsOn = diagnosticItem.depends_on;
        }
        const waveIndex = Number(diagnosticItem.canonical_wave_index);
        if (queueGraphMode && Number.isInteger(waveIndex) && waveIndex >= 0) {
          item.waveId = ensureWave(waveIndex).id;
        }
      }
      if (waves.length) queueWaves = normalizeQueueWaves(waves, queueItems, { pruneBackendEmpty: true });
    }

    function queueGraphDiagnosticItems(diagnostic) {
      const position = Number(diagnostic?.item_position);
      const matches = queueItems.filter((item, index) => {
        if (diagnostic?.item_id && item.id === diagnostic.item_id) return true;
        if (diagnostic?.item_slug && item.slug === diagnostic.item_slug) return true;
        return Number.isInteger(position) && Number(item.position ?? index) === position;
      });
      if (matches.length) return matches;
      return Number.isInteger(position) && queueItems[position] ? [queueItems[position]] : [];
    }
