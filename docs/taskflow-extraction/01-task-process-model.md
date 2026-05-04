# 01. Task Process Model and State Machine

## Goal

Create the Q-COLD-owned model for deterministic development processes. This is
the foundation for all later extraction work.

## Scope

- Add typed Rust models for `TaskProcess`, `TaskRun`, `EnvironmentRun`,
  `AgentRun`, `WorktreeAction`, `TaskOutcome`, and `TaskEvent`.
- Add state APIs over the existing SQLite `tasks`, `runs`, `events`,
  `budgets`, `claims`, and `recipes` tables.
- Define explicit states: `created`, `opening`, `worktree-ready`,
  `environment-ready`, `agent-running`, `agent-finished`,
  `validation-running`, `closeout-running`, `success`, `blocked`, `failed`,
  and `incomplete-closeout`.
- Reject invalid transitions inside Q-COLD.
- Keep existing adapter-backed command behavior unchanged.

## Dependencies

None. This package should land first.

## Parallel Work

Can run in parallel with capability-contract design notes, but not with broad
runtime changes. Runtime changes need this model first.

## Acceptance Criteria

- Q-COLD can create a task-process record without invoking a repository
  adapter.
- Q-COLD can append typed task, run, environment, and agent events.
- Invalid transitions fail with deterministic errors.
- Existing adapter-backed `task`, `verify`, `ci`, `compat`, `ffi`, `build`,
  `install`, and `bundle` commands keep working.

## Validation

- Unit tests for state transitions and invalid transition errors.
- SQLite persistence tests for task/run/event rows.
- `cargo fmt --check`
- `cargo test --locked --bin qcold --bin cargo-qcold`
