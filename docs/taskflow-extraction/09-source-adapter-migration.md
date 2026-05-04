# 09. Source Adapter Compatibility and Migration

## Goal

Turn the current source-repository task-flow implementation into a
compatibility adapter, then retire duplicated workflow ownership.

## Scope

- Add characterization tests that lock down current adapter-backed task-open,
  environment, validation, closeout, and cleanup behavior.
- Reimplement each behavior in Q-COLD-owned code behind the repository
  capability contract.
- Change legacy repository commands into thin wrappers that invoke Q-COLD.
- Keep wrapper parity until operators no longer depend on legacy entrypoints.
- Remove duplicated adapter-owned state machine code after Q-COLD is the source
  of truth.

## Dependencies

Depends on package 8 for terminal closeout ownership. Characterization tests
can start earlier.

## Parallel Work

Can run as a test/documentation lane throughout the migration. Wrapper
conversion should be the last step after the Q-COLD-owned flow is stable.

## Acceptance Criteria

- The source repository can use Q-COLD as its only task-flow driver.
- Legacy commands either delegate to Q-COLD or clearly report that the Q-COLD
  binary is required.
- Parity tests prove that supported legacy behaviors still work.
- No repository-specific proof or product validation semantics are described as
  generic Q-COLD behavior until implemented behind capabilities.

## Validation

- Characterization tests against the compatibility adapter.
- Wrapper invocation tests.
- Targeted command checks for the migrated public command surface.
- `cargo fmt --check`
- `cargo test --locked --bin qcold --bin cargo-qcold`
