# 06. Resource Governance and Admission Control

## Goal

Add resource accounting and launch admission based on task environment
boundaries.

## Scope

- Sample Docker or Podman stats for running task environments.
- Record peak memory, average and peak CPU, CPU seconds where available, pids,
  IO bytes, OOM state, start time, finish time, and sample count.
- Implement admission checks using reserved budget, live usage, host capacity,
  profile defaults, and historical peaks.
- Support admission outcomes: allow, allow with smaller profile, queue, or
  reject with a concrete reason.
- Store per-profile and per-task budgets in Q-COLD state.

## Dependencies

Depends on packages 4 and 5 for stable environment and agent run handles.

## Parallel Work

Can run in parallel with package 7 after package 5 lands. Keep writes
separated: this package owns resource sampling, budget state, and admission
decisions.

## Acceptance Criteria

- Q-COLD can explain why a new agent run is allowed, queued, or rejected.
- Resource summaries are persisted after each environment or agent run.
- Admission does not depend only on current low usage when a larger profile
  budget is reserved.
- Resource sampling degrades cleanly when the runtime cannot expose a field.

## Validation

- Unit tests for stats parsing and admission decisions.
- Fake runtime tests for OOM, missing fields, and live container sampling.
- `cargo fmt --check`
- `cargo test --locked --bin qcold --bin cargo-qcold`
