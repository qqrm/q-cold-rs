# 05. Agent Work Slice Orchestration

## Goal

Make agent execution a deterministic Q-COLD step inside an already prepared
task environment.

## Scope

- Define recipes for bounded agent work slices.
- Start agent commands through Q-COLD using a prepared environment handle.
- Record prompt, command, cwd, environment, model hints, runner metadata, and
  terminal target where applicable.
- Preserve attachable terminal support while separating terminal UI state from
  workflow ownership.
- Require agent completion before validation transitions.
- Leave failed or interrupted runs in explicit resumable state.

## Dependencies

Depends on package 4.

## Parallel Work

Can run in parallel with package 6 sampler design and package 7 parser work,
but resource admission and run-summary claims should wait until agent runs are
recorded.

## Acceptance Criteria

- Q-COLD can start one bounded agent work slice for an opened task process.
- Agent run records link to task process, environment run, prompt, logs, and
  terminal target.
- Failed and interrupted agents produce explicit states and events.
- Existing `qcold agent start` behavior remains compatible.

## Validation

- Unit tests for recipe rendering and agent-run state transitions.
- Integration tests for terminal and non-terminal agent starts.
- `cargo fmt --check`
- `cargo test --locked --bin qcold --bin cargo-qcold`
