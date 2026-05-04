# Agent Registry

`AGENTS.md` owns task flow, routing constraints, closeout, and terminal
acceptance. This file owns delegated-role choice.

Use only these fixed specialists:

- `architect_planner`
- `delivery_worker`
- `surgical_worker`
- `quality_auditor`
- `ops_guard`

## Role choice rules

- Prefer the cheapest sufficient executor.
- Use `architect_planner` first only when the task is ambiguous, moves a
  contract, spans more than two meaningful surfaces, or needs phased rollout.
- Use `delivery_worker` by default for concrete bounded implementation.
- Use `surgical_worker` only for tiny obvious one-surface edits.
- Use `quality_auditor` before closeout for public command surface, agent
  orchestration, task-flow state, process management, Telegram integration,
  CI/security, or broad diffs.
- Use `ops_guard` for CI/CD, workflow YAML, provenance, secrets, permissions,
  release automation, and operational hardening.

## Role map

### root orchestrator
Owns framing, sequencing, integration, final acceptance, canonical validation,
task-flow state validation, operator handoff, and terminal closeout.

### architect_planner
Use for decomposition, extraction slicing, contract design, rollout order,
adapter boundaries, and validation strategy.

### delivery_worker
Default bounded implementation worker for concrete multi-file code or test/doc
changes.

### surgical_worker
Tiny, obvious, one-surface edits where anything stronger would be wasteful.

### quality_auditor
Independent review for risky or release-facing work, missing validation, or
broad diffs.

### ops_guard
Worker for CI/CD, automation, provenance, permissions, and operational hardening.

## Model/profile alignment

- root orchestrator: profile `orchestrator`
- architect_planner: profile `architect_planner`
- delivery_worker: profile `repo_exec`
- surgical_worker: profile `surgical`
- quality_auditor: profile `audit`
- ops_guard: profile `ops_guard`

Every delegated ask should carry the matching qualification prompt from
`.codex/personas/`.
