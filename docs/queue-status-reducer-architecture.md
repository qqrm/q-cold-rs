# Queue Status Reducer Architecture

Queue state reconciliation is split into three layers. This keeps retries out of
status discovery and makes each queue transition explainable.

## 1. Evidence collector

The collector reads facts and does not mutate queue state. Its output is a
`QueueStatusEvidence` snapshot:

- persisted queue item/run status;
- task-record status, when visible;
- task-record sync error, when sync is unavailable;
- remote-native session liveness;
- local agent liveness;
- failed-closeout prompt liveness;
- active launch worker and recovery-worker hints.

A sync failure is evidence freshness loss, not task failure. It becomes
`StaleUnknown` with a freshness timestamp in the queue row instead of consuming a
retry budget.

## 2. Pure reducer

`reduce_queue_status` is intentionally pure. Given `QueueStatusEvidence`, it
returns:

- `effective_status`;
- a stable `reason` string;
- one `allowed_action`;
- whether this evidence fully handled the item.

The reducer never updates the database, starts an agent, stops an agent, or
increments retry counters.

| Evidence | Effective status | Allowed action |
| --- | --- | --- |
| sync unavailable | `StaleUnknown` | `RefreshEvidence` |
| no record yet, live remote session | `WaitingForRecord` | `None` |
| no record, executor vanished before record | `LaunchFailed` | `BoundedRelaunch` |
| open record + live executor/session | `Running` | `MarkRunning` or `None` |
| remote-native open record + no matching session | `DisconnectedOpenRecord` | `RelaunchRemoteDisconnectedOpenRecord` |
| local open record + no local agent | `ExecutionFailed` | `RecoverExecution` |
| `closed:success` | `ClosedSuccess` | `MarkSuccess` |
| `closed:failed` / `failed-closeout`, no live recovery | `ExecutionFailed` | `RecoverExecution` |
| `failed-closeout` + live remote session | `CloseoutFailedButSessionLive` | `MarkRunning` |
| terminal non-success | `TerminalFailure` | `MarkFailed` |

## 3. Action executor

`execute_queue_status_reduction` is the only layer that mutates queue rows for
reconciled task-record status. It may only perform the reducer's allowed action.
This makes retry policy explicit:

- `BoundedRelaunch` is for launch failures before a task/session exists;
- `RelaunchRemoteDisconnectedOpenRecord` resets a known disconnected
  remote-native open record into the remote-native relaunch path;
- `RecoverExecution` consumes semantic recovery budget;
- `RefreshEvidence` updates stale/freshness messaging and does not relaunch;
- local disconnected open records use semantic recovery, not launch retry.

## Important invariant

Retries are for known failure classes. They are not the fallback for ambiguous
status. Unknown status must remain stale/unknown until evidence improves.
