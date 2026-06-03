    function terminalForQueueItem(item, task = item ? taskRecordForQueueItem(item) : null) {
      const agentIds = queueItemAgentIds(item, task);
      const taskId = task?.id || (item?.slug ? `task/${item.slug}` : '');
      if (!agentIds.length && !taskId) return null;
      return (model?.terminals?.records || []).find((terminal) => {
        if (agentIds.includes(terminal.agent_id)) return true;
        if (taskId && terminal.scope === taskId) return true;
        return agentIds.some((agentId) => terminal.target === remoteNativeTerminalTarget(agentId));
      }) || null;
    }

    function remoteNativeTerminalTarget(agentId) {
      return agentId ? `remote-tmux:${agentId}:0.0` : '';
    }

    function terminalForTaskId(taskId) {
      const task = queueTaskRecords().find((record) => record.id === taskId);
      const queueItem = queueItems.find((item) => `task/${item.slug}` === taskId);
      return terminalForQueueItem(queueItem || null, task || null)
        || (model?.terminals?.records || []).find((terminal) => terminal.scope === taskId)
        || null;
    }

    function terminalForTarget(target) {
      if (!target) return null;
      return (model?.terminals?.records || []).find((terminal) => terminal.target === target) || null;
    }
