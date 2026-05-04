# Q-COLD-Owned Task-Flow Extraction Backlog

This backlog captures the planned migration from adapter-owned task-flow
procedures to deterministic Q-COLD-owned development processes. It is planning
material, not a claim that the behavior is already implemented.

## Target Shape

Q-COLD should own the workflow state machine. Repository adapters should expose
typed capabilities that Q-COLD invokes in a fixed order.

The target control loop is:

1. Create or resume a task process.
2. Prepare the primary checkout and managed worktree.
3. Prepare or reuse the task environment.
4. Run one bounded agent work slice.
5. Capture resource, token, prompt, diff, and validation telemetry.
6. Run deterministic validation and closeout transitions.
7. Persist events, summaries, bundles, and cleanup state.

Agents should not be responsible for remembering workflow sequencing. They
should receive bounded work prompts inside a prepared environment, while
Q-COLD owns transitions, admission checks, and terminal outcomes.

## Work Packages

### 1. Task Process Model and State Machine

Define Q-COLD-owned types for task processes, task runs, environment runs,
agent runs, and terminal outcomes.

Scope:

- Add typed models for `TaskProcess`, `TaskRun`, `EnvironmentRun`,
  `AgentRun`, `WorktreeAction`, `TaskOutcome`, and `TaskEvent`.
- Map the existing SQLite `tasks`, `runs`, `events`, `budgets`, `claims`, and
  `recipes` tables to explicit Rust APIs.
- Make transitions explicit: `created`, `opening`, `worktree-ready`,
  `environment-ready`, `agent-running`, `agent-finished`,
  `validation-running`, `closeout-running`, `success`, `blocked`, `failed`,
  and `incomplete-closeout`.
- Reject invalid transitions at the Q-COLD layer instead of relying on agent
  discipline.

Acceptance:

- Q-COLD can create a task-process record and append typed events without
  invoking a repository adapter.
- State transition tests cover valid and invalid paths.
- Existing adapter-backed commands keep their current behavior.

### 2. Repository Capability Contract

Replace black-box task-flow delegation with a typed capability boundary.

Scope:

- Define an adapter contract for repository-specific facts and actions:
  approved base branch, managed worktree root, profile resolution, worktree
  bootstrap, environment config rendering, validation commands, and cleanup
  hooks.
- Keep string-command execution as a compatibility adapter, but make Q-COLD
  call typed capabilities internally.
- Add machine-readable capability inspection so `qcold repo inspect` can show
  what a repository supports.
- Document the minimum repository contract required for Q-COLD-owned task
  open.

Acceptance:

- A repository can report task-flow capabilities without running task open.
- Missing capabilities fail with actionable errors.
- The current xtask process adapter remains usable during migration.

### 3. Q-COLD-Owned Task Open Core

Move the deterministic task-open sequence into Q-COLD while keeping
repository-specific preparation behind capabilities.

Scope:

- Resolve and validate task slugs and task branch names.
- Acquire a per-repository/per-task lock.
- Require a clean orchestration checkout before mutating task state.
- Fetch and fast-forward the approved base branch.
- Select `create`, `resume`, `reattach`, or `resume-from-remote`.
- Create or reattach the managed worktree.
- Generate task env, execution anchor, task ids, and task-open events.
- Return a machine-readable `TaskOpenResult`.

Acceptance:

- Q-COLD can execute the task-open decision core in tests without delegating to
  adapter-level `task open`.
- Existing regression fixtures cover create, resume, reattach, dirty primary,
  and resume-from-remote behavior.
- Host-side task worktrees remain orchestration substrates, not approved
  places for substantive agent work.

### 4. Environment and Container Lifecycle

Lift devcontainer/container handling into a Q-COLD-owned environment layer.

Scope:

- Add `EnvironmentProfile` and `EnvironmentHandle` models.
- Discover Docker or Podman containers by Q-COLD task labels.
- Render effective environment configs through repository capabilities.
- Start, reuse, recheck, and recreate task environments deterministically.
- Persist container id, container name, image, profile, runtime, and labels.
- Support profile-level resource declarations: memory, CPU, pids, privileged
  mode, and runtime-specific options.

Acceptance:

- Q-COLD can identify the running container for a task process by labels.
- Q-COLD records the environment handle in its run state.
- Environment startup emits typed events and clear failure reasons.
- Profile resource declarations are visible before an agent is admitted.

### 5. Agent Work Slice Orchestration

Make agent execution a deterministic step inside an already prepared task
environment.

Scope:

- Define recipes for bounded agent work slices.
- Start agent commands through Q-COLD using a prepared environment handle.
- Record the exact prompt, command, cwd, environment, model hints, and runner
  metadata.
- Preserve attachable terminal support while separating terminal UI state from
  workflow ownership.
- Require agent completion before validation transitions.

Acceptance:

- Q-COLD can start one bounded agent slice for an opened task process.
- Agent run records link to task process, environment run, prompt, logs, and
  terminal target where applicable.
- Failed or interrupted agents leave explicit resumable state.

### 6. Resource Governance and Admission Control

Add a resource accounting layer based on environment/container boundaries.

Scope:

- Sample Docker or Podman stats for running task environments.
- Record peak memory, average and peak CPU, CPU seconds where available, pids,
  IO bytes, OOM state, start time, finish time, and sample count.
- Implement admission checks using reserved budget, live usage, host capacity,
  profile defaults, and historical peaks.
- Support outcomes: allow, allow with smaller profile, queue, or reject with a
  concrete resource reason.
- Store per-profile and per-task budget records in Q-COLD state.

Acceptance:

- Q-COLD can explain why a new agent run is allowed or denied.
- Resource summaries are persisted after each task environment or agent run.
- No admission decision depends only on current low usage when a larger
  resource limit is already reserved.

### 7. Prompt, Token, Diff, and Efficiency Telemetry

Unify task result telemetry under Q-COLD-owned run summaries.

Scope:

- Import or capture prompt archives and prompt metadata.
- Capture agent usage: input tokens, cached input tokens, output tokens, total
  tokens, credits, runner, model, agent id, status, and remaining capacity.
- Capture git result metrics: files changed, lines added, lines deleted,
  touched top-level areas, binary changes, and generated files.
- Link validation outcome and closeout outcome to the same run summary.
- Compute derived signals such as tokens per accepted changed line, resource
  cost per successful task, retry count, and validation-fix rate.

Acceptance:

- Task summaries can be produced without reading adapter-private logs.
- Missing telemetry is represented as partial or unavailable with a reason.
- Efficiency metrics are descriptive signals, not automatic quality judgments.

### 8. Closeout, Bundles, and Cleanup Ownership

Move terminal workflow completion into Q-COLD-owned transitions.

Scope:

- Model finalize, success closeout, blocked closeout, failed closeout, and
  incomplete closeout explicitly.
- Run validation profiles through Q-COLD-selected plans with repository
  capability hooks.
- Preserve evidence bundles for blocked, failed, and incomplete outcomes.
- Clean task worktrees, containers, images, and stale residues only through
  deterministic policy.
- Keep cleanup conservative when other open task processes exist.

Acceptance:

- Q-COLD can report whether a task reached terminal completion.
- Bundles include task env, run summary, events, prompt metadata, resource
  summary, validation summary, and terminal receipt.
- Incomplete closeout never masquerades as success.

### 9. Source Adapter Compatibility and Migration

Turn the current source-repository task-flow implementation into a compatibility
adapter, then retire duplicated workflow ownership.

Scope:

- Add characterization tests that lock down the current source-adapter task
  open behavior.
- Reimplement each behavior in Q-COLD-owned code behind the new capability
  contract.
- Change legacy repository commands into thin wrappers that invoke Q-COLD.
- Keep wrapper parity until operators no longer depend on legacy entrypoints.
- Remove duplicated adapter-owned state machine code after Q-COLD is the
  source of truth.

Acceptance:

- The source repository can use Q-COLD as its only task-flow driver.
- Legacy commands either delegate to Q-COLD or clearly report that the Q-COLD
  binary is required.
- No repository-specific proof or product validation semantics are described as
  generic Q-COLD behavior until implemented behind capabilities.

## Suggested Sequencing

1. Land the task process model and transition tests.
2. Land the repository capability contract and inspect command.
3. Extract task-open decision core into Q-COLD.
4. Add environment/container lifecycle ownership.
5. Add agent work slice orchestration.
6. Add resource governance and admission control.
7. Add prompt/token/diff/resource run summaries.
8. Move closeout and bundle ownership.
9. Convert source repository wrappers to Q-COLD-only flow.

This order keeps Q-COLD honest: it first owns state and transitions, then owns
container handles, then owns resource and result accounting, and only then
becomes the terminal closeout authority.
