# 03. Q-COLD-Owned Task Open Core

## Goal

Move the deterministic task-open decision core into Q-COLD while keeping
repository-specific preparation behind capabilities.

## Scope

- Resolve and validate task slugs and task branch names.
- Acquire a per-repository/per-task lock.
- Require a clean orchestration checkout before task mutation.
- Fetch and fast-forward the approved base branch.
- Select `create`, `resume`, `reattach`, or `resume-from-remote`.
- Create or reattach the managed worktree.
- Generate task env, execution anchor, task ids, and task-open events.
- Return a typed `TaskOpenResult`.

## Dependencies

Depends on packages 1 and 2.

## Parallel Work

Can run in parallel with package 9 characterization tests. Container lifecycle
work should wait until this package has stable task env and worktree results.

## Acceptance Criteria

- Q-COLD can execute the task-open decision core without delegating to
  adapter-level `task open`.
- Tests cover create, resume, reattach, dirty primary, and resume-from-remote.
- Task-open events are persisted in Q-COLD state.
- Host-side task worktrees remain orchestration substrates, not approved
  places for substantive agent work.

## Validation

- Task-open unit tests and regression fixtures.
- Targeted command checks for `qcold task open --help` or the equivalent
  staged command surface.
- `cargo fmt --check`
- `cargo test --locked --bin qcold --bin cargo-qcold`
