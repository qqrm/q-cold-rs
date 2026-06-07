const QcoldQueueStatus = (() => {
  const QueueRun = Object.freeze({
    Running: 'running',
    Waiting: 'waiting',
    Starting: 'starting',
    Stopping: 'stopping',
    Stopped: 'stopped',
    Failed: 'failed',
    Success: 'success',
  });

  const QueueItem = Object.freeze({
    Pending: 'pending',
    Waiting: 'waiting',
    Starting: 'starting',
    Running: 'running',
    Stopping: 'stopping',
    Stopped: 'stopped',
    Paused: 'paused',
    Success: 'success',
    Failed: 'failed',
    Blocked: 'blocked',
  });

  const QueueView = Object.freeze({
    Idle: 'idle',
  });

  const QueueExecutionMode = Object.freeze({
    Graph: 'graph',
    Sequence: 'sequence',
  });

  const TaskRecord = Object.freeze({
    Open: 'open',
    Paused: 'paused',
    ClosedSuccess: 'closed:success',
    ClosedFailed: 'closed:failed',
    ClosedBlocked: 'closed:blocked',
    FailedCloseout: 'failed-closeout',
  });

  const queueRunActive = Object.freeze([
    QueueRun.Running,
    QueueRun.Waiting,
    QueueRun.Starting,
    QueueRun.Stopping,
  ]);
  const queueRunAppendable = Object.freeze([
    QueueRun.Running,
    QueueRun.Waiting,
    QueueRun.Starting,
    QueueRun.Stopped,
  ]);
  const queueRunTerminal = Object.freeze([QueueRun.Success, QueueRun.Failed]);
  const queueItemTerminal = Object.freeze([QueueItem.Success, QueueItem.Failed, QueueItem.Blocked]);
  const queueItemActive = Object.freeze([
    QueueItem.Starting,
    QueueItem.Running,
    QueueItem.Waiting,
    QueueItem.Stopping,
  ]);
  const queueItemStartingOrRunning = Object.freeze([QueueItem.Starting, QueueItem.Running]);
  const queueItemPendingOrWaiting = Object.freeze([QueueItem.Pending, QueueItem.Waiting]);
  const queueItemStoppedOrPaused = Object.freeze([QueueItem.Stopped, QueueItem.Paused]);
  const queueItemExecutorSession = Object.freeze([QueueItem.Starting, QueueItem.Running, QueueItem.Stopping]);
  const queueItemFailedOrBlocked = Object.freeze([QueueItem.Failed, QueueItem.Blocked]);

  function statusValue(value) {
    if (value && typeof value === 'object') return String(value.status || '');
    return String(value || '');
  }

  function statusIn(value, statuses) {
    return statuses.includes(statusValue(value));
  }

  function isTaskRecordClosed(value) {
    return statusValue(value).startsWith('closed');
  }

  function hasFailedStatus(value) {
    return statusValue(value).includes(QueueItem.Failed);
  }

  function hasBlockedStatus(value) {
    return statusValue(value).includes(QueueItem.Blocked);
  }

  function taskRecordClosedStatus(value) {
    const status = statusValue(value);
    return isTaskRecordClosed(status) ? status : '';
  }

  return Object.freeze({
    QueueRun,
    QueueItem,
    QueueView,
    QueueExecutionMode,
    TaskRecord,
    statusValue,
    hasFailedStatus,
    hasBlockedStatus,
    isQueueRunActive: (value) => statusIn(value, queueRunActive),
    isQueueRunAppendable: (value) => statusIn(value, queueRunAppendable),
    isQueueRunEditable: (value) => statusIn(value, queueRunAppendable),
    isQueueRunTerminal: (value) => statusIn(value, queueRunTerminal),
    isQueueRunStarting: (value) => statusValue(value) === QueueRun.Starting,
    isQueueRunStopped: (value) => statusValue(value) === QueueRun.Stopped,
    isQueueRunSuccess: (value) => statusValue(value) === QueueRun.Success,
    isQueueRunFailed: (value) => statusValue(value) === QueueRun.Failed,
    isQueueItemActive: (value) => statusIn(value, queueItemActive),
    isQueueItemTerminal: (value) => statusIn(value, queueItemTerminal),
    isQueueItemStarting: (value) => statusValue(value) === QueueItem.Starting,
    isQueueItemPending: (value) => statusValue(value) === QueueItem.Pending,
    isQueueItemSuccess: (value) => statusValue(value) === QueueItem.Success,
    isQueueItemFailed: (value) => statusValue(value) === QueueItem.Failed,
    isQueueItemBlocked: (value) => statusValue(value) === QueueItem.Blocked,
    isQueueItemStopped: (value) => statusValue(value) === QueueItem.Stopped,
    isQueueItemPaused: (value) => statusValue(value) === QueueItem.Paused,
    isQueueItemStartingOrRunning: (value) => statusIn(value, queueItemStartingOrRunning),
    isQueueItemPendingOrWaiting: (value) => statusIn(value, queueItemPendingOrWaiting),
    isQueueItemStoppedOrPaused: (value) => statusIn(value, queueItemStoppedOrPaused),
    queueItemHasExecutorSession: (value) => statusIn(value, queueItemExecutorSession),
    isQueueItemFailedOrBlocked: (value) => statusIn(value, queueItemFailedOrBlocked),
    isTaskRecordOpen: (value) => statusValue(value) === TaskRecord.Open,
    isTaskRecordPaused: (value) => statusValue(value) === TaskRecord.Paused,
    isTaskRecordClosed,
    isTaskRecordClosedSuccess: (value) => statusValue(value) === TaskRecord.ClosedSuccess,
    isTaskRecordClosedFailed: (value) => statusValue(value) === TaskRecord.ClosedFailed,
    isTaskRecordClosedBlocked: (value) => statusValue(value) === TaskRecord.ClosedBlocked,
    taskRecordClosedStatus,
  });
})();

const QcoldQueueRunStatus = QcoldQueueStatus.QueueRun;
const QcoldQueueItemStatus = QcoldQueueStatus.QueueItem;
const QcoldQueueViewStatus = QcoldQueueStatus.QueueView;
const QcoldQueueExecutionMode = QcoldQueueStatus.QueueExecutionMode;
const QcoldTaskRecordStatus = QcoldQueueStatus.TaskRecord;
