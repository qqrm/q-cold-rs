# 07. Prompt, Token, Diff, and Efficiency Telemetry

## Goal

Unify prompt, token, diff, validation, and efficiency telemetry under
Q-COLD-owned run summaries.

## Scope

- Import or capture prompt archives and prompt metadata.
- Capture agent usage: input tokens, cached input tokens, output tokens, total
  tokens, credits, runner, model, agent id, status, and remaining capacity.
- Capture git result metrics: files changed, lines added, lines deleted,
  touched top-level areas, binary changes, and generated files.
- Link validation outcome and closeout outcome to the same run summary.
- Compute descriptive signals: tokens per accepted changed line, resource cost
  per successful task, retry count, and validation-fix rate.

## Dependencies

Depends on package 5 for agent run records. Resource-related summary fields
depend on package 6.

## Parallel Work

Can start parser and data-model work after package 4. Full run-summary
integration should wait for package 5. Resource fields can be integrated in
parallel with package 6 through stable summary structs.

## Acceptance Criteria

- Task summaries can be produced without reading adapter-private logs.
- Missing telemetry is represented as partial or unavailable with a reason.
- Efficiency metrics are descriptive signals, not automatic quality judgments.
- Summary schemas are versioned and covered by tests.

## Validation

- Unit tests for prompt metadata, token usage, diff stats, and partial
  telemetry.
- Golden tests for run-summary JSON.
- `cargo fmt --check`
- `cargo test --locked --bin qcold --bin cargo-qcold`
