# 04. Environment and Container Lifecycle

## Goal

Lift environment and container handling into Q-COLD-owned runtime state.

## Scope

- Add `EnvironmentProfile` and `EnvironmentHandle` models.
- Discover Docker or Podman containers by Q-COLD task labels.
- Render effective environment configs through repository capabilities.
- Start, reuse, recheck, and recreate task environments deterministically.
- Persist container id, container name, image, profile, runtime, and labels.
- Support profile declarations for memory, CPU, pids, privileged mode, and
  runtime-specific options.

## Dependencies

Depends on package 3. Uses package 2 capabilities for repository-specific
environment details.

## Parallel Work

Can run in parallel with pure telemetry data-structure work from package 7.
Do not wire agent execution or resource admission until environment handles are
persisted.

## Acceptance Criteria

- Q-COLD can identify the running container for a task process by labels.
- Q-COLD records the environment handle in run state.
- Environment startup emits typed events and clear failure reasons.
- Profile resource declarations are visible before an agent is admitted.
- Existing adapter-backed environment flow still works during migration.

## Validation

- Unit tests for Docker/Podman label parsing and container selection.
- Regression tests for environment create, reuse, recheck, and recreate.
- Targeted command checks for environment inspection where available.
- `cargo fmt --check`
- `cargo test --locked --bin qcold --bin cargo-qcold`
