# 08. Closeout, Bundles, and Cleanup Ownership

## Goal

Move terminal workflow completion into Q-COLD-owned transitions.

## Scope

- Model finalize, success closeout, blocked closeout, failed closeout, and
  incomplete closeout explicitly.
- Run validation profiles through Q-COLD-selected plans with repository
  capability hooks.
- Preserve evidence bundles for blocked, failed, and incomplete outcomes.
- Include task env, run summary, events, prompt metadata, resource summary,
  validation summary, and terminal receipt in bundles.
- Clean task worktrees, containers, images, and stale residues only through
  deterministic policy.
- Keep cleanup conservative when other open task processes exist.

## Dependencies

Depends on packages 6 and 7. Uses package 2 capabilities for validation and
repository-specific cleanup hooks.

## Parallel Work

Can run in parallel with package 9 wrapper characterization. Do not convert
legacy wrappers until this package can prove terminal outcomes.

## Acceptance Criteria

- Q-COLD can report whether a task reached terminal completion.
- Incomplete closeout never masquerades as success.
- Evidence bundles are written for blocked, failed, and incomplete outcomes.
- Cleanup policy is deterministic and preserves investigative residue when
  required.

## Validation

- Regression tests for success, blocked, failed, and incomplete closeout.
- Bundle content tests for receipts, summaries, events, and telemetry.
- Targeted command checks for closeout help and outcome validation.
- `cargo fmt --check`
- `cargo test --locked --bin qcold --bin cargo-qcold`
