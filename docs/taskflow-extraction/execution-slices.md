# Task-Flow Extraction Execution Slices

This file turns the numbered migration packages into bounded implementation
slices. It is planning material. Status notes below are inferred from the
current command surface, source tree, and regression coverage in this
repository.

## Current Snapshot

- Package 1 is partially landed: the SQLite schema already has `runs`,
  `events`, `claims`, `budgets`, and `recipes`, but Q-COLD still lacks a
  first-class typed task-process state machine.
- Package 2 is not landed: repository registration exists, but there is no
  typed capability contract or `qcold repo inspect` surface yet.
- Packages 3 and 4 are the strongest landed areas: `task open`, managed
  worktrees, and devcontainer lifecycle already have regression coverage, but
  the ownership boundary still leans on adapter-era behavior.
- Package 5 is only partially landed: `qcold agent start` exists, but agent
  execution is not yet modeled as a Q-COLD-owned task-flow step linked to
  task/environment run state.
- Package 6 is largely unlanded: the schema has `budgets`, but there is no
  live sampling or admission engine in the runtime path.
- Packages 7 and 8 are partially landed through bundle and closeout coverage,
  but the telemetry and terminal-state model are still not clearly separated
  from compatibility behavior.
- Package 9 is the final migration lane and is not landed: the xtask process
  adapter is still the compatibility backbone for task-flow commands.

## Recommended Order

1. Finish package 2 enough to create a real capability boundary.
2. Pull package 3 task-open decisions behind that boundary.
3. Normalize package 4 environment handles so agent runs can reference stable
   runtime state.
4. Land package 5 task-linked agent runs.
5. Run packages 6 and 7 in parallel once package 5 records the required run
   metadata.
6. Land package 8 closeout ownership after 6 and 7.
7. Keep package 9 parity tests accumulating, then convert wrappers last.

## Bounded Slices

### Slice A: Capability Model Skeleton

- Add typed capability structs for base branch, worktree root, supported task
  profiles, environment config roots, validation lanes, and cleanup hooks.
- Teach the xtask process adapter to populate that model as a compatibility
  source instead of exposing only opaque command execution.
- Add `qcold repo inspect` with deterministic missing-capability errors.
- Lock this down with unit tests before touching broader task-open logic.

### Slice B: Q-COLD Task-Open Decision Core

- Introduce a typed `TaskOpenResult` with action=`create|resume|reattach|resume-from-remote`.
- Move branch naming, primary cleanliness checks, base fast-forward, lock
  acquisition, and worktree selection into Q-COLD-owned code.
- Persist task-open events in the existing SQLite state instead of treating the
  adapter as the sole source of truth.
- Preserve current user-visible `task open` stdout/stderr contracts with
  regression fixtures.

### Slice C: Stable Environment Handles

- Add typed environment profile and environment handle models in Rust.
- Persist runtime, container id/name, selected profile, image ref, and labels
  into run state.
- Make environment discovery and reattach use Q-COLD-owned state first, with
  runtime inspection as verification rather than the only source of truth.
- Keep current devcontainer behavior and profile selection outputs stable.

### Slice D: Agent Runs As Task-Flow State

- Add a Q-COLD-owned task agent-run record that links task, environment, cwd,
  command, runner, model hint, logs, and terminal target.
- Make bounded task work slices start through this model instead of treating
  `qcold agent start` as a side channel.
- Add explicit interrupted/failed/resumable run states.
- Preserve standalone `qcold agent start` compatibility while task-linked runs
  grow behind the new state path.

### Slice E: Resource Sampling And Admission

- Define a runtime-neutral sampler interface for Docker/Podman stats.
- Store per-task and per-profile reservations in the existing `budgets` table.
- Add admission outcomes `allow`, `shrink-profile`, `queue`, and `reject` with
  concrete reasons.
- Start with fake runtime tests and deterministic decision tests before wiring
  live sampling into launch paths.

### Slice F: Q-COLD-Owned Run Summaries

- Version the task run-summary schema explicitly.
- Move prompt metadata, token usage, diff stats, validation result, and
  telemetry availability into one Q-COLD-owned summary builder.
- Mark partial telemetry with concrete reasons instead of silently omitting
  fields.
- Keep bundle paths and operator-facing closeout summaries stable while the
  implementation source changes underneath.

### Slice G: Terminal Closeout Ownership

- Add explicit Q-COLD transitions for `finalize`, `success`, `blocked`,
  `failed`, and `incomplete-closeout`.
- Make validation planning and terminal outcome reporting run through the
  capability contract.
- Preserve the current conservative cleanup rules and blocked/incomplete bundle
  behavior proven by regression tests.
- Do not delete compatibility wrapper behavior until Q-COLD can prove terminal
  ownership end to end.

### Slice H: Compatibility Wrapper Conversion

- Add parity tests that pin current adapter-backed command behavior for open,
  enter, closeout, cleanup, and bundle surfaces.
- Convert legacy repository entrypoints into thin wrappers that delegate to
  Q-COLD.
- Remove duplicated adapter-owned workflow state only after parity tests stay
  green on the new path.
- Keep repository-specific proof semantics behind capabilities; do not let them
  leak into generic Q-COLD docs.

## Good First Tickets

- `repo inspect`: add command surface plus one compatibility capability report.
- typed `TaskOpenResult`: extract current open outcomes into one Rust enum and
  persistence path.
- environment handle persistence: write profile/runtime/container identity into
  SQLite on successful task open.
- task-linked agent run row: record one bounded run with logs and terminal
  target.
- run-summary versioning: freeze the JSON schema already emitted in bundles.

## What Not To Do Yet

- Do not start resource admission before task-linked agent runs exist.
- Do not convert legacy wrappers before Q-COLD owns terminal closeout state.
- Do not describe repository-specific validation lanes as generic Q-COLD
  behavior until they are capability-backed.
