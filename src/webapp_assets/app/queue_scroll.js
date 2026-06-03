    function captureQueueWaveScrollPositions() {
      const positions = new Map(queueWaveScrollPositions);
      queueStatus.querySelectorAll('.queue-graph-wave').forEach((wave) => {
        const waveId = wave.dataset.waveId || '';
        const lane = wave.querySelector('.queue-graph-wave-lane');
        if (waveId && lane) positions.set(waveId, lane.scrollLeft);
      });
      return positions;
    }

    function restoreQueueWaveScrollPositions(positions) {
      queueWaveScrollPositions.clear();
      queueStatus.querySelectorAll('.queue-graph-wave').forEach((wave) => {
        const waveId = wave.dataset.waveId || '';
        const lane = wave.querySelector('.queue-graph-wave-lane');
        if (!waveId || !lane || !positions.has(waveId)) return;
        queueWaveScrollPositions.set(waveId, positions.get(waveId));
      });
      window.requestAnimationFrame(() => {
        queueStatus.querySelectorAll('.queue-graph-wave').forEach((wave) => {
          const waveId = wave.dataset.waveId || '';
          const lane = wave.querySelector('.queue-graph-wave-lane');
          if (!waveId || !lane || !queueWaveScrollPositions.has(waveId)) return;
          lane.scrollLeft = Math.min(
            queueWaveScrollPositions.get(waveId),
            Math.max(0, lane.scrollWidth - lane.clientWidth),
          );
        });
      });
    }
