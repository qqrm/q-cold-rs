    function terminalForTaskId(taskId) {
      const task = queueTaskRecords().find((record) => record.id === taskId);
      const queueItem = queueItems.find((item) => `task/${item.slug}` === taskId);
      const agentId = queueItemAgentId(queueItem, task);
      if (!agentId) return null;
      return (model?.terminals?.records || []).find((terminal) => terminal.agent_id === agentId) || null;
    }

    function terminalForTarget(target) {
      if (!target) return null;
      return (model?.terminals?.records || []).find((terminal) => terminal.target === target) || null;
    }
