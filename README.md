# Q-COLD

Q-COLD is a Rust orchestration facade for agent-driven task-flow work. It owns
the operator CLI, local dashboard, queue control plane, task records, agent
session registry, and repository adapter boundary.

Repository-specific validation, proof, CI, and closeout semantics stay behind
explicit adapters. The first adapter is the generic `xtask` process adapter; the
Q-COLD repo also ships its own `xtask` adapter for dogfooding.

![Q-COLD web dashboard terminals](docs/screenshots/qcold-web-terminals.jpg)

## Install

Install or refresh the standalone operator binary:

```bash
qcold install
```

`qcold <command>` is the primary interface. `cargo qcold <command>` remains for
Cargo subcommand compatibility.

Check the installed build:

```bash
qcold --version
qcold --help
```

The version includes the Cargo package version, commit-count build number, and
Git hash. Dirty local rebuilds append `-dirty`.

## Register A Repo

Register a repository that exposes an `xtask` adapter:

```bash
qcold repo add target /path/to/repo \
  --xtask-manifest /path/to/repo/xtask/Cargo.toml \
  --default-branch main \
  --set-active
qcold repo list
qcold status
```

Adapter-backed commands use the active repo unless a command accepts an explicit
repo root. Keep target-repo behavior behind that adapter; Q-COLD should not grow
repository-specific validation rules directly.

## Task Flow

Open local managed work:

```bash
qcold task open my-task
qcold task enter my-task
qcold task closeout --outcome success --message "Validated and delivered."
```

Open remote managed work while keeping local Q-COLD state canonical:

```bash
qcold task open-remote --via remote-dev-env my-remote-task
```

Use `qcold task pause --reason "<reason>"` for a non-terminal wait. Use terminal
blocked/failed closeout only when the task is actually stopped.
Successful, blocked, and failed terminal closeouts remove their managed task
worktrees after writing the terminal bundle. Health and stale-cleanup commands
also prune leftover `closed:*` task worktrees, metadata-only direct-child task
directories whose task branch is gone, top-level detached managed worktrees with
no `.task/task.env`, and stale Git worktree metadata. `open`, `paused`, and
`failed-closeout` worktrees are preserved when a repairable Git worktree still
exists because they still need resume, operator action, or closeout repair.

## Queue

The queue can run prompt packages, dependency graphs, and follow-up tasks through
the dashboard daemon:

```bash
qcold queue run --from queue.json --agent c1 --repo-root /path/to/repo
qcold queue append <run-id> --prompt "follow-up task"
qcold queue list
qcold queue stop
qcold queue continue <run-id>
qcold queue clear --run-id <run-id>
```

Local queue launches that use `c1` or `c2` select between those two commands at
agent startup from the daemon's cached readiness probes. Probes refresh in the
background about every 30 minutes; when both eligible agents are limited, the
item remains waiting with the next retry time from the status cache.

Queue tabs isolate active runs:

```bash
qcold queue create "client queue"
qcold queue switch <queue-tab-id>
qcold queue delete <queue-tab-id>
```

A `queue run` request that reuses an unfinished slug, including retry-shaped
variants such as new `after-repair-p<port>` suffixes, is rejected before a new
run is persisted; use `queue append`, `queue continue`, or `queue clear` for
intentional recovery.
Each queue item keeps a durable semantic attempt ledger. Q-COLD uses it to cap
semantic work at three total iterations per item: the original attempt plus two
auto-recovery attempts. Launch retries for agent startup remain separate from
that semantic cap. Failed task closeout, `closed:failed`, and local executor
exit before closeout all route through that bounded recovery path with a fresh
executor and prior-failure context.
Queue item workers also take a durable SQLite lease with a heartbeat before
launching executor work, so a restarted daemon can distinguish active ownership
from expired ownership and retry bounded stale work without relying only on
process-local thread state.
Queue items also carry a task class: `cheap`, `mid`, or `heavy`; omitted class
fields default to `mid`. Graph scheduling admits ready items against live queue
reservations plus the last hour of local host resource samples. The default
8-core / 128 GiB policy has soft max 8 tasks, hard max 12 tasks, and heavy max
2 tasks. Items that cannot be admitted stay `waiting` with an admission reason
and `next_attempt_at` retry time.
Queue run, append, and update dashboard API responses preserve the existing
`ok`/`output` fields and may include `queue_graph` diagnostics with canonical
dependency normalization, wave indexes, and display-safe validation messages.
Local rows with a matching open task record but no live agent session use the
same bounded auto-recovery path, starting a fresh executor with the prior task
record, logs, and bundle context instead of stopping for operator resume. Older
stopped rows with that missing-agent message are reconciled into the same
recovery path. Remote-native stopped rows retain their remote agent identity for
remote resume.

Mutating queue commands post to the local dashboard daemon on
`127.0.0.1:8787` by default. If it is not reachable, Q-COLD starts it unless the
command is passed `--no-start-daemon`.

Remote-native rows are reconciled against task records, terminal bundles, and
live remote tmux sessions. A stale `failed-closeout` record is shown as running
while the same remote-native agent session is still alive. An `open`
remote-native record without a live remote-agent session is relaunched through
the bounded remote-native retry path.
Remote task-record sync is bounded by
`QCOLD_REMOTE_TASK_RECORD_SYNC_TIMEOUT_SECONDS`, defaulting to 30 seconds, so a
stale remote launcher cannot freeze queue reconciliation.
The dashboard daemon periodically reconciles queue rows against task records
once per minute by default. Set `QCOLD_WEB_QUEUE_STATUS_SYNC_INTERVAL_SECONDS`
to tune that interval.
If remote-agent launch succeeds but the remote task record and tmux session are
not visible, Q-COLD schedules a bounded relaunch instead of leaving a terminal
failed row. Remote port-forward failures run a best-effort `remote-agent down`
before rotating to the next candidate remote proxy port.
Queue rows with a live executor terminal open the task chat modal with the
latest bounded terminal tail, including remote-native tmux panes captured
through the configured remote launcher.
Rows without a live executor terminal but with `.task/logs/agent-execution.md`
show that visible task-flow log immediately instead of waiting on Codex session
metadata refresh.

## Node Snapshot

Collect a typed monitoring snapshot for the current node:

```bash
qcold node snapshot --pretty
```

Fetch the same protocol from a running dashboard node:

```bash
qcold node snapshot --endpoint http://127.0.0.1:8787 --pretty
```

The dashboard also serves the typed payload at `/api/node/snapshot` and embeds
it in `/api/state` as `node`. The snapshot includes managed agents, terminal
sessions, queue visibility, proxy and port-forward state, heartbeat metadata,
and basic CPU, load, memory, swap, disk, pid, IO, and network counters. Blocks
carry `fresh`, `stale`, `partial`, or `unavailable` status so missing host data
is visible to clients.

## Dashboard

Start the local dashboard:

```bash
qcold telegram serve --listen 127.0.0.1:8787 --daemon
```

Open `http://127.0.0.1:8787`.

The dashboard serves repository status, task records, queues, managed agents,
terminals, and transcripts. It keeps a background state snapshot and pushes
updates through server-sent events, so routine sync does not require frontend
page reloads. The browser also keeps a bounded `/api/state` watcher active and
refreshes immediately when the page regains focus or network connectivity, so a
stale tab does not need an F5 reload to catch up.

Local dashboard startup does not require access to the Telegram CDN. The
browser only loads the Telegram WebApp SDK asynchronously when Telegram launch
parameters are present, or uses an already-present `window.Telegram.WebApp`.

Dashboard writes can require an operator token:

```bash
QCOLD_WEBAPP_REQUIRE_WRITE_TOKEN=1 \
QCOLD_WEBAPP_WRITE_TOKEN='<secret>' \
qcold telegram serve --listen 127.0.0.1:8787 --daemon
```

The daemon checks `X-QCOLD-Write-Token` on mutating dashboard requests. It does
not embed `QCOLD_WEBAPP_WRITE_TOKEN` in served HTML or JavaScript; enter the
token in the dashboard header for the current browser session.

After rebuilding or reinstalling Q-COLD, restart the daemon so the served binary
and embedded web assets match:

```bash
qcold install
qcold telegram serve --listen 127.0.0.1:8787 --daemon
```

WSL autostart:

```bash
qcold wsl autostart install --listen 127.0.0.1:8787
qcold wsl autostart status
qcold wsl autostart remove
```

## Agents

List and manage local agent sessions:

```bash
qcold agent list
qcold agent start --track audit -- c1 "inspect repo"
qcold agent attach <agent-id|terminal-target|session|name>
```

Advanced maintenance commands such as task-record CRUD, bundles, output guard,
adapter pass-through lanes, named-session cleanup, and stale-agent pruning remain
available for compatibility but are intentionally hidden from default help.
Source bundle commands require a clean checkout and fast-forward from the
configured upstream, when one exists, before archiving. Terminal closeout
bundles preserve task evidence as-is and do not run a pre-bundle sync.

## Validation

Local preflight:

```bash
cargo xtask verify fast
```

The fast gate checks tracked text hygiene, formatting, web asset syntax, unit
tests, Clippy policy, and stable integration suites that do not require external
fixtures. Heavier gates:

```bash
cargo xtask verify full
cargo xtask verify task-flow
```

For Rust code changes, also run:

```bash
cargo fmt --check
cargo test --locked
```

## Development Contract

`AGENTS.md` is the authoritative development contract for task open/closeout,
language policy, delegation, validation, and repository ownership. Keep README
operator-facing and concise; put workflow rules in `AGENTS.md` and executable
behavior claims in tests.

The current Rust include-boundary inventory is tracked in
`docs/include-boundaries.md`.
