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
  commands delegate through the explicit xtask process adapter. Q-COLD owns a
  repository-local self adapter under `xtask/` for dogfooding its own flow. If
  that adapter or another target repository's adapter prerequisites are absent,
  use normal Cargo validation and report that task-flow closeout was not
  applicable.
- Do not bypass the public command surface when the task is explicitly about
  Q-COLD command behavior. Exercise `cargo qcold ...` through the compiled
  binary or `cargo run -- ...` as appropriate.
- If resuming after interruption, reread this file and any task-local logs
  before touching code.
- Treat the system prompt, developer prompt, `.codex/config.toml`, this file,
  and the nearest local `AGENTS.md` as hard constraints. Resolve conflicts by
  instruction precedence.

## Language Policy

- Keep all visible agent-authored interim output in English, including status
  updates, plans, reasoning summaries, review notes, handoff notes, task-flow
  control messages, delegated asks, commit messages, and repository artifacts.
- The only routine exception is the final operator-facing summary at the end of
  the task: write that final chat response in Russian.
- If an explicit task deliverable requires another language, limit that
  language to the requested deliverable while keeping surrounding agent
  workflow communication in English until the final Russian summary.

## Response Style

- Keep operator-facing agent responses short, dry, and task-focused.
- Prefer compact status, decisions, validation, blockers, and next steps over
  broad explanation.
- Avoid cheerleading, filler, rhetorical framing, and decorative phrasing unless
  needed for correctness or blocker context.
- Expand only when the operator asks for detail or when concise detail is
  required to prevent ambiguity or misuse.

## Public Command Surface

- `cargo install --path . --locked`
- `qcold --version`
- `qcold --help`
- `qcold status`
- `qcold task-record list`
- `qcold task-record create --description "<task description>"`
- `qcold task-record show <task-id>`
- `qcold task-record update <task-id> [--title "<title>"] [--description "<description>"] [--status <status>]`
- `qcold task-record close <task-id> [--outcome success|blocked|failed]`
- `qcold task-record delete <task-id>`
- `qcold agent list`
- `qcold agent attach <agent-id|terminal-target|session|name>`
- `qcold agent start --track <track> -- <command>...`
- `qcold telegram poll`
- `qcold tui`
- `qcold wsl autostart install [--listen <addr>] [--repo-root <path>] [--qcold-bin <path>]`
- `qcold wsl autostart status`
- `qcold wsl autostart remove`
- `qcold bundle`
- `qcold guard -- <command>...`
- `qcold task pause --reason "<reason>"`
- `qcold repo list`
- `qcold repo add <id> <root> [--adapter xtask-process] [--xtask-manifest <path>] [--set-active]`
- `qcold repo set-active <id>`
- `cargo qcold --help`
- `cargo qcold --version`
- `cargo qcold status`
- `cargo qcold task-record list`
- `cargo qcold task-record create --description "<task description>"`
- `cargo qcold task-record show <task-id>`
- `cargo qcold task-record update <task-id> [--title "<title>"] [--description "<description>"] [--status <status>]`
- `cargo qcold task-record close <task-id> [--outcome success|blocked|failed]`
- `cargo qcold task-record delete <task-id>`
- `cargo qcold agent list`
- `cargo qcold agent attach <agent-id|terminal-target|session|name>`
- `cargo qcold agent start --track <track> -- <command>...`
- `cargo qcold telegram poll`
- `cargo qcold tui`
- `cargo qcold wsl autostart install [--listen <addr>] [--repo-root <path>] [--qcold-bin <path>]`
- `cargo qcold wsl autostart status`
- `cargo qcold wsl autostart remove`
- `cargo qcold guard -- <command>...`
- `cargo qcold task inspect [topic]`
- `cargo qcold task open <task-slug> [profile]`
- `cargo qcold task enter`
- `cargo qcold task list`
- `cargo qcold task terminal-check`
- `cargo qcold task iteration-notify --message "<handoff update>"`
- `cargo qcold task pause --reason "<reason>"`
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
- Shape broad searches and log reads before consuming raw output. Use focused
  paths, `rg -l`, `rg --count`, `sed -n`, `head`, `tail`, or
  `qcold guard -- <command>...` when a command can dump large output.
- Update docs in the same task when behavior, command contracts, environment
  variables, or operator expectations change.
- Do not invent validation, closeout, Telegram delivery, or deployment status.
- Treat technical validation and closeout blockers as agent-fixable when the
  fix is small or mechanical and stays within the task's technical scope.
  Formatting, lint, helper visibility, harness drift, and similarly code-local
  issues should be repaired and revalidated in the same managed state. Business
  behavior choices, disputed contracts, missing operator context, credentials,
  and external resources are honest blockers or pause reasons, not agent
  guesswork.

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
  closeout when that closeout surface is available. For Q-COLD self-development,
  prefer the self-hosted `QCOLD_REPO_ROOT=$PWD cargo qcold task open
  <task-slug>` flow from a clean primary checkout unless this repository is
  already the active registered repo, and close out from the managed worktree.
  If managed task-flow prerequisites are absent or failing, commit after normal
  Cargo validation and report that task-flow closeout was not applicable.
- `task pause` is non-terminal: it records why the attempt is waiting and keeps
  the managed worktree/devcontainer for direct resume. `blocked` remains
  terminal and is reserved for tasks that are being stopped for operator or
  external resolution. Stale paused self-hosted task state is eligible for
  automatic cleanup after `QCOLD_PAUSED_TASK_TTL_HOURS` or 2 hours by default;
  ZIP bundles are retained separately for `QCOLD_BUNDLE_RETENTION_HOURS` or 24
  hours by default.
- For Q-COLD self-development, successful managed closeout fetches `origin`,
  fast-forwards the primary checkout to the current remote base, rebases the
  task branch onto that base, fast-forward integrates it into the primary base
  branch, pushes that base branch to `origin`, refreshes the remote-tracking
  ref, and only then marks the task terminal. A local-only commit is not a
  successful terminal closeout when the push-capable managed flow is available.
- Do not perform final operator installation from a task branch or managed task
  worktree. After successful integration into `main`, rebuild and install
  Q-COLD only from the primary checkout so the installed binary reflects landed
  repository state.

## Validation Authority

- Every non-trivial iteration should pass `cargo xtask verify fast` locally
  before terminal closeout. The repository-local `xtask` implementation is the
  local preflight entry point, and Q-COLD self-hosted `cargo qcold verify` plus
  successful task closeout invoke it through the same adapter boundary.
- The self-hosted fast gate enforces tracked text hygiene before heavier
  validation: new tracked text files must stay at or below 1,000 lines, all
  tracked text lines must stay at or below 120 characters, and any large-file
  exception must be explicit in `xtask/src/quality.rs` with a reason.
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

In operator-facing summaries, the first sentence must state whether the
changes are integrated and pushed to the target branch. If integration or push
did not happen, report that first as non-terminal or local-only state. Mention
notable task-flow or tooling problems and mitigations when they occurred.
