# Task-Flow Extraction Roadmap

The migration should preserve the current adapter-backed command surface while
Q-COLD gradually takes ownership of deterministic workflow state, environment
handles, agent runs, telemetry, and closeout.

## Critical Path

1. Land [01-task-process-model.md](01-task-process-model.md).
2. Land [02-repository-capability-contract.md](02-repository-capability-contract.md).
3. Land [03-task-open-core.md](03-task-open-core.md).
4. Land [04-environment-container-lifecycle.md](04-environment-container-lifecycle.md).
5. Land [05-agent-work-slice-orchestration.md](05-agent-work-slice-orchestration.md).
6. Land [06-resource-governance.md](06-resource-governance.md).
7. Land [07-run-telemetry.md](07-run-telemetry.md).
8. Land [08-closeout-bundles-cleanup.md](08-closeout-bundles-cleanup.md).
9. Land [09-source-adapter-migration.md](09-source-adapter-migration.md).

The first three packages are the foundation. Do not start broad implementation
of container lifecycle, agent execution, or closeout ownership until Q-COLD has
a task-process model, a capability contract, and a Q-COLD-owned task-open core.

## Parallel Lanes

After package 1 lands, package 2 can proceed in parallel with design-only
preparation for packages 6 and 7. Do not wire resource or telemetry code into
runtime paths until package 4 creates stable environment handles.

After package 2 lands, package 3 can proceed while a separate worker prepares
characterization fixtures for package 9. The fixtures should not change legacy
behavior; they only lock down parity.

After package 3 lands, package 4 is the main blocker for packages 5 and 6.
Package 7 can start with pure parsers and summary data structures in parallel,
but it should not claim full run-summary ownership until package 5 records
agent runs.

After package 5 lands, packages 6 and 7 can run in parallel:

- package 6 owns live resource sampling and admission decisions;
- package 7 owns prompt, token, diff, validation, and efficiency summaries.

Package 8 should wait for both package 6 and package 7, because closeout
bundles must include resource and run summaries.

Package 9 is the final integration lane. It can keep accumulating parity tests
throughout the migration, but it should only convert legacy wrappers after
packages 3, 4, 5, and 8 are stable.

## Suggested Fanout

Keep root ownership of sequencing and integration. Use at most two concurrent
implementation lanes before each integration checkpoint:

- Lane A: critical path implementation.
- Lane B: characterization tests, docs, or a non-blocking parser/model slice.

Avoid parallel edits to the same modules. Good split examples:

- state machine implementation vs. repository capability docs/tests;
- container discovery code vs. telemetry parser data structures;
- closeout bundle schema tests vs. legacy wrapper characterization tests.

## Checkpoints

After each package:

- keep adapter-backed behavior working;
- update user-facing docs only for behavior that has landed;
- run the relevant targeted command checks;
- keep migration status explicit in README or this directory;
- do not describe repository-specific validation semantics as generic Q-COLD
  behavior until they are implemented behind capabilities.

## Target End State

Q-COLD owns the deterministic flow:

1. prepare task process;
2. prepare worktree;
3. prepare environment;
4. run bounded agent work;
5. capture telemetry;
6. validate;
7. close out;
8. clean up.

Repositories provide capabilities. Agents perform bounded code-changing work.
Q-COLD owns ordering, state, resource admission, telemetry, and terminal
outcomes.
