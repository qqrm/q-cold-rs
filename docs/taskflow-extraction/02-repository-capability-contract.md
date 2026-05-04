# 02. Repository Capability Contract

## Goal

Replace black-box task-flow delegation with a typed repository capability
boundary. Q-COLD should own sequencing; repositories should expose facts and
actions.

## Scope

- Define a capability contract for repository-specific facts:
  approved base branch, managed worktree root, supported profiles, environment
  config locations, validation lanes, and cleanup hooks.
- Define typed capability outcomes instead of relying only on string output.
- Keep the current process adapter as a compatibility implementation.
- Add capability inspection so operators can see what the active repository
  supports before running task flow.
- Document the minimum repository requirements for Q-COLD-owned task open.

## Dependencies

Depends on package 1 for task-process state references.

## Parallel Work

Can proceed in parallel with characterization fixture planning for package 9.
Do not convert legacy wrappers yet.

## Acceptance Criteria

- `qcold repo inspect` or an equivalent command can report repository
  task-flow capabilities without opening a task.
- Missing capabilities fail with actionable errors.
- Existing registered repositories still work through the compatibility
  adapter.
- Capability tests cover both supported and unsupported repositories.

## Validation

- Unit tests for capability parsing and missing-capability errors.
- Targeted command check for repository inspection.
- `cargo fmt --check`
- `cargo test --locked --bin qcold --bin cargo-qcold`
