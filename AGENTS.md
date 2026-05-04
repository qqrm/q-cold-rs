# Q-COLD Development and Agent Contract

This file is the single repository truth for Q-COLD development flow,
delegation, validation, and closeout.

Q-COLD is currently an extracted Rust orchestration facade with an incubating
repository adapter. Keep that boundary explicit: do not describe
adapter-backed repository behavior as standalone Q-COLD behavior until the
repo-local code owns it.

## Mandatory Preflight

- Start by checking `git status --short` and reading the smallest relevant
  slice: this file, `README.md`, and directly touched code or tests.
- Keep the primary checkout clean unless the current task is actively changing
  tracked files.
- When a managed task-flow environment is available for this checkout, start
  tracked work from the primary checkout with
  `cargo qcold task open <task-slug> [profile]` and complete it through the
  task closeout surface.
- During incubation, task, verify, ci, compat, ffi, build, and install
  commands delegate through the explicit xtask process adapter. If
  local Q-COLD development lacks the adapter prerequisites, remote, or
  devcontainer surface, use normal Cargo validation in this repository and
  report that task-flow closeout was not applicable.
- Do not bypass the public command surface when the task is explicitly about
  Q-COLD command behavior. Exercise `cargo qcold ...` through the compiled
  binary or `cargo run -- ...` as appropriate.
- If resuming after interruption, reread this file and any task-local logs
  before touching code.
- Treat the system prompt, developer prompt, `.codex/config.toml`, this file,
  and the nearest local `AGENTS.md` as hard constraints. Resolve conflicts by
  instruction precedence.

## Public Command Surface

- `cargo install --path . --locked`
- `qcold --help`
- `qcold status`
- `qcold agent list`
- `qcold agent start --track <track> -- <command>...`
- `qcold telegram poll`
- `qcold bundle`
- `qcold repo list`
- `qcold repo add <id> <root> [--adapter xtask-process] [--xtask-manifest <path>] [--set-active]`
- `qcold repo set-active <id>`
- `cargo qcold --help`
- `cargo qcold status`
- `cargo qcold agent list`
- `cargo qcold agent start --track <track> -- <command>...`
- `cargo qcold telegram poll`
- `cargo qcold task inspect [topic]`
- `cargo qcold task open <task-slug> [profile]`
- `cargo qcold task enter`
- `cargo qcold task list`
- `cargo qcold task terminal-check`
- `cargo qcold task iteration-notify --message "<handoff update>"`
- `cargo qcold task finalize --message "<message>"`
- `cargo qcold task closeout --outcome success --message "<commit message>"`
- `cargo qcold task closeout --outcome blocked --reason "<reason>"`
- `cargo qcold task closeout --outcome failed --reason "<reason>"`
- `cargo qcold task clean <task-slug>`
- `cargo qcold task clear <task-slug>`
- `cargo qcold task clear-all`
- `cargo qcold task orphan-list`
- `cargo qcold task orphan-clear-stale [--max-age-hours <hours>]`
- `cargo qcold bundle`
- `cargo qcold repo list`
- `cargo qcold repo add <id> <root> [--adapter xtask-process] [--xtask-manifest <path>] [--set-active]`
- `cargo qcold repo set-active <id>`
- `cargo qcold verify ...`
- `cargo qcold ci ...`
- `cargo qcold compat ...`
- `cargo qcold ffi ...`

The standalone `qcold` binary is the primary operator-facing surface.
`cargo qcold` remains supported as Cargo subcommand compatibility. Direct
calls into a repository adapter are implementation details unless a test
fixture or debugging task explicitly needs them.

## Development Rules

- Keep Q-COLD Rust-first and facade-owned: command parsing, operator UX,
  process exit behavior, Telegram control-plane adapters, agent registry
  behavior, and adapter boundaries belong here.
- Keep repository-specific proof, validation, and closeout semantics behind the
  adapter boundary until they are deliberately extracted.
- Preserve Cargo subcommand invocation behavior: `cargo qcold ...` and direct
  `cargo-qcold ...` execution must stay equivalent where intended.
- Prefer small, typed Rust changes over stringly shell glue.
- Avoid broad refactors unless the task is explicitly architectural.
- Update docs in the same task when behavior, command contracts, environment
  variables, or operator expectations change.
- Do not invent validation, closeout, Telegram delivery, or deployment status.

## Iteration Closeout Discipline

- Unless the user explicitly asks to stop before persistence, non-blocked work
  that changes tracked state must end in a reviewed local commit on the current
  branch.
- Do not leave a finished implementation as an uncommitted diff. If validation
  is blocked or the change is intentionally not committed, say that explicitly
  and keep the reason visible in the final summary.
- Keep commits scoped to the task. Do not include local secrets, runtime state,
  generated logs, or unrelated user edits.
- During incubation, a local commit is not a substitute for managed task-flow
  closeout when that closeout surface is available. If managed task-flow
  prerequisites are absent or failing, commit after normal Cargo validation and
  report that task-flow closeout was not applicable.

## Validation Authority

- For Rust code changes, run `cargo fmt --check` and `cargo test --locked`
  unless the task is too narrow or the environment blocks them.
- For command-surface changes, include targeted help or invocation checks such
  as `cargo run --bin qcold -- --help`, `cargo run --bin qcold -- task --help`,
  or the specific command being changed.
- For task-flow control-plane changes, run the relevant regression tests:
  `cargo test --test task_flow_control_plane --locked` and/or
  `cargo test --test task_flow_regression --locked`.
- `cargo qcold task closeout --outcome success` is terminal task completion
  only when the managed task-flow prerequisites are present and the command
  reaches terminal success. Green local Cargo tests are not task-flow closeout.
- If a validation command is skipped or fails for environmental reasons, say so
  explicitly in the final summary.

## Delegation and Routing

The root agent owns framing, sequencing, integration, final acceptance,
validation choice, task-flow state checks, operator handoff, and closeout.

Use only the fixed specialists defined in `.codex/AGENT_REGISTRY.md`:

- `architect_planner`
- `delivery_worker`
- `surgical_worker`
- `quality_auditor`
- `ops_guard`

Routing rules:

- keep `max_depth = 1`
- keep parallel fanout small: root plus at most two worker shards before first
  integration
- give each worker a disjoint write scope
- prefer the cheapest sufficient executor
- use `architect_planner` first only when the task is ambiguous, changes a
  contract, spans more than two meaningful surfaces, or needs phased rollout
- default bounded implementation to `delivery_worker`
- use `surgical_worker` only for tiny obvious one-surface edits
- use `quality_auditor` for public command surface, agent orchestration,
  task-flow state, process management, adapter boundaries, CI/security,
  Telegram behavior, or broad diffs
- use `ops_guard` for CI/CD, provenance, secrets, permissions, release
  automation, and operational hardening
- when delegating, carry the matching qualification prompt from
  `.codex/personas/`

Delegated asks should stay in English unless the explicit task output requires
another language.

## Documentation Rules

- Root `AGENTS.md` owns development flow, routing, validation, and closeout.
- `README.md` owns user-facing installation, command examples, environment
  variables, and current incubation status.
- Tests own executable behavior claims.
- Docs must describe landed repository truth, not aspirational behavior.
- Remove stale experiments and historical logs instead of keeping them as live
  guidance.

## Final Summaries

In operator-facing summaries, state whether the changes are committed and on
which branch. Mention notable task-flow or tooling problems and mitigations
when they occurred.
