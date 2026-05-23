---
name: repo-task-run-audit
description: Audit managed task-flow runs, bundles, validation phases, agent records, and closeout outcomes.
---

# Repository Task Run Audit

## Objective

Reconstruct the dynamic history of managed tasks from current state and durable
artifacts. The goal is to explain what happened, why it happened, and what the
next operational action should be without inventing validation or closeout
status.

## Start With Live State

1. Read the repo contract first: root `AGENTS.md`, `README.md`, nearest local
   `AGENTS.md`, and `.task/task.env` when present.
2. Check `git status --short --branch` in the active worktree.
3. Use the public surface for task and agent state:
   `cargo qcold task list`, `cargo qcold task terminal-check`,
   `cargo qcold task-record list`, `cargo qcold task-record audit`,
   and `cargo qcold agent list` when applicable.
4. If the user shows an agent launch command such as `cc1 name="meta"`, verify
   whether that value is Q-COLD metadata, a terminal pane title, or merely an
   argument passed into the wrapped agent. Do not assume it renamed the
   Q-COLD task record or Codex session.

Why: live state tells whether a bundle is stale, superseded, or still blocking
terminal closeout. Agent names and terminal names often come from different
systems, so they must be checked separately.

## Audit Bundles

When bundle paths are provided or a repo has a `bundles/` directory, make a
timeline before reading raw logs:

```bash
python3 .codex/skills/repo-task-run-audit/scripts/summarize_task_bundles.py \
  /path/to/repo/bundles/*.zip
```

Then inspect the smallest relevant files inside each bundle:

- `metadata/task.env`: task id, sequence, profile, base/head commits, final
  status, delivered head, and merged head.
- `metadata/task-run-summary.json`: normalized phases, durations, exit codes,
  highlights, failure tails, telemetry, and prompt metadata.
- `logs/flow-problems.md` and `logs/flow-problems.json`: grouped failures and
  recurring phase problems.
- `logs/summary/*.json` and short `logs/*.log` summaries: phase-level proof
  without dumping raw logs.
- `evidence/git-status.txt`, `evidence/index.patch`, and
  `evidence/working-tree.patch`: whether closeout left uncommitted state.
- `metadata/terminal-receipt.env`: failed, blocked, or success closeout
  receipts, especially `CURRENT_FLOW_PROBLEM`, `HISTORICAL_FLOW_PROBLEM`,
  `CLOSEOUT_FAILURE_PHASE`, and `CLOSEOUT_FAILURE_ERROR` when present.
- Domain artifacts such as `logs/compat/**/summary.tsv`,
  `logs/compat/**/regressions.tsv`, and targeted repro logs when the task was
  about compatibility or migration proof.
- Source archives such as `repo-<hash>-source.zip`: commit snapshots that can
  anchor code state, but do not prove task outcome by themselves.

Why: task bundles preserve the state seen by closeout. Prefer structured
summary files first; raw logs are for confirming a specific failing phase,
test, or operational blocker.

## Compare Outcomes

For repeated task names or sequence numbers, compare failed or blocked bundles
against later success bundles:

- `STATUS`, `TASK_HEAD`, `DELIVERED_HEAD`, and `MERGED_HEAD`.
- failed phase names, exit codes, and `failure_tail`.
- whether later bundles have empty evidence patches.
- whether validation phases that failed earlier now have `exit_code=0`.
- whether the same task sequence generated multiple bundles.

Why: a later success bundle can make an earlier failed-closeout bundle
historical, but the earlier failure still explains what was fixed or retried.

## Classify Findings

Use conservative labels:

- `integrated`: success closeout reached terminal success and records a merged
  or delivered head.
- `local-only`: code changed or validation ran, but task-flow success closeout
  did not complete.
- `blocked`: task ended terminal blocked due to missing prerequisites,
  external fixtures, credentials, baseline data, or unresolved behavioral
  decisions.
- `failed-closeout`: closeout attempted success but validation, integration,
  or cleanup failed; look for later bundles before calling it final.
- `superseded`: an older bundle was replaced by a later bundle for the same
  task sequence or branch.

Why: these labels separate repository truth from task-flow mechanics. Do not
convert "tests passed" into "integrated" unless closeout evidence also supports
it.

## Report

Keep the operator-facing result compact:

- Timeline: task sequence, slug, timestamp, outcome, head, profile.
- What changed dynamically: failed to success, blocked prerequisites, profile
  mismatches, missing baselines, validation lane failures, or no matching
  Codex rollout telemetry.
- Evidence pointers: bundle file and the exact internal file that supports the
  claim.
- Next action: rerun closeout, fix a validation failure, open a follow-up task,
  clean stale task state, or leave a blocker for operator input.

When memory or previous run summaries are used, say that they are background
context and still verify drift-prone facts against current repo state or bundle
contents.
