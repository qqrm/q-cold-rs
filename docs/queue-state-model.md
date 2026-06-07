# Queue State Model

This is the target model for queue scheduler tests. It is not a claim that the
runtime scheduler fully implements every rule yet.

## Counters

- `semantic_iteration`: one repository task attempt that reaches task-flow
  execution. The original attempt counts as iteration 1.
- `launch_retry`: a pre-task executor launch retry. It does not consume a
  semantic iteration.
- `admission`: the scheduler decision that a ready item may own one live
  executor slot.
- `task_class`: the item resource class: `cheap`, `mid`, or `heavy`. Missing
  classes default to `mid`.

## Item States

- `pending`: dependencies and retry timers decide whether the item is ready.
- `admitted`: the scheduler has reserved a launch slot for this item.
- `starting`: executor setup is in progress.
- `running`: a local agent or remote-native session owns the item.
- `waiting_launch_retry`: executor setup failed before task-flow execution.
- `waiting_semantic_retry`: task-flow execution failed and another semantic
  iteration remains.
- `stopped`: operator pause or disconnected remote-native open record.
- `success`, `failed`, `blocked`: terminal item states.

## Target Invariants

1. A failed item gets at most three total semantic iterations: original plus two
   semantic recoveries.
2. Launch retries are tracked separately from semantic iterations.
3. Restart reconciliation must attach to an already live executor instead of
   launching a duplicate or incrementing attempts.
4. Repeating reconciliation over the same persisted state is idempotent.
5. Graph mode must honor admission when picking ready items, so a ready item
   that is already admitted cannot be launched again.

The runtime scheduler admits graph-ready items against live reservations and
last-hour host resource samples. Rows that cannot be admitted remain `waiting`
with the admission reason and next retry time.
