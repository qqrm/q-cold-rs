# Codex Hooks Pilot

Status: design and inventory only. This repository does not currently install project hooks:
`.codex/hooks.json` is absent, no hook wrapper exists, and `qcold codex-hook` is not implemented.

The pilot should not move Q-COLD task flow into Codex hooks. Task open, validation, closeout,
bundling, cleanup, agent routing, and repository-specific proof stay owned by `qcold`, the
self-hosted `xtask` adapter, and `AGENTS.md`. Hooks should only make selected high-cost runtime
rules deterministic.

## Inventory

- `AGENTS.md`: authoritative for development flow, delegation, validation, and closeout.
- `.codex/config.toml`: sets the repo-local agent defaults and repeats the hard workflow contract.
- `.codex/AGENT_REGISTRY.md`: owns delegated role choice; hooks should not select or spawn agents.
- `.codex/hooks.json`: absent. The dispatcher task introduces the first project hook layer.
- `src/lib.rs`: owns the public and hidden `qcold` command surface. The dispatcher belongs here as a
  hidden top-level command, not as adapter pass-through behavior.
- `src/repository.rs`: already resolves primary checkout, managed worktree, active repo, and cwd
  mismatch state. Hook code can reuse or mirror this classification, but must not weaken adapter
  checks that already guard task-sensitive commands.
- `xtask/src/main.rs`: owns self-hosted task open, enter, list, terminal-check, pause, closeout,
  bundles, and cleanup. Hooks may surface context before those commands, but must not duplicate the
  terminal closeout state machine.
- `xtask/src/task/env_io.rs`: defines the task metadata fields hooks need from `.task/task.env`.
  Future hook code should parse only the small field set it needs in Q-COLD-owned Rust code.
- `xtask/src/task/bundle.rs`: copies `.task/logs` into terminal bundles. Future hook telemetry under
  `.task/logs/codex-hooks.ndjson` will be preserved by the existing bundle copy path.
- Existing test homes: command visibility can fit `tests/command_version.rs`; task/worktree fixture
  behavior can fit `tests/task_flow_control_plane.rs` or a focused hook dispatcher integration test
  using `tests/support/task_flow_helpers.rs`.

## Dispatcher Contract

- Command name: add hidden top-level `qcold codex-hook`.
- Wrapper path: add `.codex/hooks/qcold-codex-hook.sh`.
- Wrapper behavior: POSIX shell only; locate the repository root, then execute an installed `qcold`
  binary or an already-built local binary. It must pass stdin through and must not implement policy.
- JSON behavior: parse hook input and emit hook output in Rust with `serde_json`.
- Unknown event: exit zero with no output.
- Malformed JSON: fail open; write only a capped diagnostic to stderr.
- Hot path budget: no broad scans, validation, Cargo builds, network calls, or adapter proof logic.
  Target sub-250 ms once a usable binary already exists.

## Minimal State Model

The dispatcher needs one cheap local state object:

- hook event name, session id, turn id, and tool name when present
- `cwd` from hook input or current process cwd
- git root from `git rev-parse --show-toplevel`
- current branch from `git branch --show-current`
- whether `.task/task.env` exists at the git root
- task fields from `.task/task.env`: task id, task name, task profile, base branch, status,
  primary repo path, task worktree, devcontainer name, delivery mode, Codex thread id, and rollout path
- checkout classification: primary checkout, managed task worktree, agent worktree, or unknown
- next legal task-flow action, such as open task, switch to task worktree, validate, pause, or closeout

Treat missing or unreadable state as unknown during the pilot. Unknown state should produce context,
not a hard block, except for explicit destructive command patterns that do not need repository state.

## Failure Policy

- Context hooks fail open.
- Telemetry hooks fail open and never block a completed tool.
- `PreToolUse` hard blocks must stay narrow and high-confidence.
- Stop continuation is disabled for the first landing. Start with observe-only output.
- Emergency bypass may be added later as `QCOLD_CODEX_HOOK_BYPASS=1`, but only if documented,
  test-covered, and visible in telemetry.

## Hook Surface

- `SessionStart`: v1 adds compact task context. Later versions may add resumed validation and blocker
  hints. No hard block. Needs git root, branch, and task env. Test primary and managed fixtures.
- `SubagentStart`: v1 adds shorter role and task context. Later versions may include a persona
  reminder only when agent type is known. No hard block. Needs task env and optional agent type.
- `UserPromptSubmit`: not enabled in v1. Later versions may refresh context after a low-noise
  dispatcher rollout. No hard block. Uses the same state as session context.
- `PreToolUse`: v1 observes suspicious primary-checkout writes. V2 may deny primary writes, raw
  destructive git, and wrong-context closeout. Needs cwd class, tool name, command, or paths. Test
  both allow and deny fixtures.
- `PostToolUse`: v1 is no-op or emits a task-open context hint. Later versions append capped
  task-local NDJSON telemetry. No hard block. Needs task env, tool response, and capped output.
- `Stop`: v1 emits an observe-only closeout nudge. Later versions may continue with a loop guard
  after manual testing. Needs task env, cheap git status, and summary/log presence.
- `PreCompact` / `PostCompact`: not enabled in v1. Later versions may preserve task id, branch,
  validation state, and blockers. No hard block. Needs task env and recent local markers.

## Implementation Checklist

1. Land the observe-only dispatcher and `.codex/hooks.json`.
2. Prove fixture JSON invocation through `qcold codex-hook` before relying on Codex runtime hooks.
3. Add high-confidence `PreToolUse` blockers with explicit allowlist tests.
4. Add task-local telemetry as `.task/logs/codex-hooks.ndjson`.
5. Add observe-only Stop nudge; enable continuation only after low-noise manual checks.
6. Update `AGENTS.md` and `README.md` only after behavior is implemented, so the docs describe landed
   repository truth.

## Rollout Notes

After `.codex/hooks.json` lands, operators must inspect and trust the project hooks with `/hooks`.
Changed hook definitions will need trust review again. A bad rollout should be disabled through the
Codex hook trust UI first, then by disabling hooks in user config, then by reverting the hook config
and dispatcher changes.
