# Q-COLD

Q-COLD is an extracted orchestration facade for agent-driven task-flow work.
The name expands to QQRM Collaboration, Orchestration, Lifecycle, and Delivery.

![Q-COLD web dashboard terminals](docs/screenshots/qcold-web-terminals.jpg)

Q-COLD owns the operator-facing command surface and keeps repository-specific
proof, validation, and closeout semantics behind explicit adapter traits. The
first adapter is a generic xtask process adapter, so Q-COLD can build without
a Cargo path dependency on any target repository. Q-COLD also ships its own
repository-local `xtask` adapter so Q-COLD development can dogfood the same
`qcold task ...`, `qcold verify ...`, and `qcold ci ...` surfaces. Install or
refresh the standalone operator binary locally with:

```bash
qcold install
```

Then register repository connections in Q-COLD state and run adapter-backed
commands through the active repository:

```bash
qcold repo add target-repo /path/to/target-repo \
  --xtask-manifest /path/to/target-repo/xtask/Cargo.toml \
  --default-branch main \
  --set-active
qcold repo list
qcold status
qcold task-record create --description "Add task CRUD and automatic capture"
qcold task-record list
qcold task-record audit --top 10
qcold task-record sync-remote --via remote-dev-env \
  --local-repo-root /path/to/local/target-repo \
  --remote-repo-root /path/to/remote/target-repo
qcold q-help
qcold queue run --from queue.json --agent c1 --repo-root /path/to/target-repo
qcold queue list
qcold queue create "client queue"
qcold queue switch <queue-tab-id>
qcold queue append <run-id> --prompt "follow-up task"
qcold queue delete <queue-tab-id>
qcold agent list
qcold agent named-sessions list --agent cc1
qcold agent named-sessions drop --agent cc1 --name atomic
qcold agent named-sessions drop-all --agent cc1
qcold agent prune-stale
qcold agent start --track audit -- c1 "inspect repo"
qcold agent start --terminal --attach --track c2 -- c2 "work on the active task"
qcold telegram poll
qcold telegram serve --listen 127.0.0.1:8787 --daemon
qcold wsl autostart install
qcold bundle
qcold guard -- rg -n "needle" src
qcold task inspect runtime-audit
qcold task open my-task
qcold task open-remote --via remote-dev-env my-remote-task
qcold task pause --reason "waiting for operator decision"
```

Legacy `cargo qcold <command>` remains supported for compatibility, but
`qcold <command>` is the primary operator interface.
Use `qcold --version` to check the installed operator binary. The reported
version includes the Cargo package version, a monotonic Git commit-count build
number, and the Git commit hash embedded when that binary was built. The build
number is commit-derived so the same clean commit reports the same version
wherever Q-COLD is installed. Remote task hosts normally run the repository
adapter, not a second Q-COLD control plane. A tracked-dirty local rebuild
appends `-dirty` to the hash so changed-but-uncommitted operator binaries
remain distinguishable.

## Local iteration checks

Run the same preflight gate locally before closing a task:

```bash
cargo xtask verify fast
```

The gate checks the tracked text quality surface, Rust formatting, web asset
JavaScript syntax, binary unit tests, Clippy correctness/suspicious/perf lints
for production binaries, and stable integration suites that do not require the
external task-flow fixture adapter. Tracked text files must stay at or below
1,000 lines and 120
characters per line unless `xtask` carries an explicit reviewed exception for
pre-existing split debt. `qcold verify` and successful managed task
closeout run the same repository-local `xtask` implementation through the
adapter boundary. Optional heavier profiles are available when the local
environment has the required fixtures:

```bash
cargo xtask verify full
cargo xtask verify task-flow
```

The root devcontainer is intentionally E2E-capable by default. It builds the
`devcontainer-e2e` target, carries task-flow test dependencies such as `7z`,
`jq`, and SSH/rsync tooling, and installs Docker-outside-of-Docker support when
the host provides it. The slim Rust-only target remains available at
`.devcontainer/slim/devcontainer.json` for explicit lightweight CI or smoke
jobs that do not need E2E tooling.

## Task records

Q-COLD stores lightweight task records in its local SQLite database. Use
`qcold task-record create`, `list`, `show`, `update`, `close`, and `delete` for
direct CRUD. When a manual or managed task-flow record has a repository root,
`create` assigns a stable repo-scoped `sequence` number and returns the existing
number on later idempotent creates for the same task id. Sequence counters are
monotonic per repo and are not reused after task-record deletion, so retained
closeout bundles keep a stable anchor history. Ad-hoc `agent` and
`codex-session` records remain repo-attributed for dashboards and transcript
lookup, but they do not consume task sequence numbers. Descriptions are
normalized before storage so operator phrasing is kept as a concise task
description instead of a raw chat transcript.

Adapter-backed `qcold task open <slug>` automatically creates or updates a
Q-COLD task record with source `task-flow`. When that record has a repo-scoped
`sequence`, Q-COLD passes it to the repository adapter as
`QCOLD_TASK_SEQUENCE` so managed task anchors can use an operator-sortable
monotonic number instead of a random-looking suffix. The self-hosted adapter
also records that value as `TASK_SEQUENCE` in `.task/task.env` so adapter-owned
evidence files can preserve both a numeric task id and the human task id/name.
Queue-opened task records
also preserve the original queue-card prompt in metadata and pass it to the
adapter as `QCOLD_TASKFLOW_PROMPT`; compact operator surfaces use a bounded
first-lines snippet instead of prompt-derived labels. Q-COLD-managed agent starts also
create an ad-hoc task record when the wrapped `c1`, `cc1`, `c2`, `cc2`, or
`codex` command contains an explicit prompt argument. Interactive prompts typed
later inside an already-open terminal are imported from Codex session JSONL
telemetry under `~/.codex-accounts/<slot>/sessions` when task records, agent
lists, or the web dashboard are refreshed. Imported sessions are attributed to
the repository that owns the launched agent cwd, including agent worktrees under
`../WT/<repo>/agents/`, so `cc1 resume ...` history appears with the target
repository instead of the dashboard daemon repository. Local `cc1` and `c1`
wrappers are treated as Codex account `1`; local `cc2` and `c2` wrappers are
treated as Codex account `2`.
The refresh path reconciles managed task-flow records from `.task/task.env`
before importing ad-hoc Codex sessions, preserving `TASK_ID`,
`TASK_SEQUENCE`, and the managed worktree as the authoritative task identity.
If a repository closes a managed task through its own task-flow command and the
worktree is gone before Q-COLD refreshes, Q-COLD also reconciles terminal
status and recorded runtime from recent `metadata/terminal-receipt.env`
closeout bundle receipts under that repository's `bundles/` directory.
The importer reads Codex `session_meta`, matches sessions to the
Q-COLD agent start time and active repository cwd, and does not assign a
claimed `session_path` to another agent. It stores the polished first
meaningful user prompt plus the latest Codex token counters in task metadata.
For adapter-backed task-flow records, Q-COLD also refreshes compact Codex token
telemetry from matching session JSONL files while task records or the dashboard
are loaded. `qcold task open` forwards the current Codex `CODEX_THREAD_ID` and
resolved rollout JSONL path to the repository adapter when available; the
self-hosted task adapter records them in `.task/task.env`, and telemetry import
tries that explicit rollout before falling back to session matching. The
fallback matcher assigns a session when `session_meta.cwd`, a structured
tool-call `workdir`/`cwd`, a structured command containing the managed path, or
a task-worktree environment marker such as `TASK_WORKTREE` is under the managed
worktree; task slug text is kept only as a diagnostic counter. It then sums
Codex `last_token_usage` samples into per-task `token_usage` metadata, stores
the matching `session_path` for transcript viewing, and stores bounded
`token_efficiency` metadata for session counts plus the largest tool outputs by
reported `Original token count`.
Q-COLD keeps only metadata, not raw tool output, and limits task-flow session
import plus new ad-hoc Codex-session imports to the recent Codex telemetry
window. The default retention window is 48 hours; set
`QCOLD_CODEX_TELEMETRY_RETENTION_HOURS` to another positive hour count for one
process. The metadata is refreshed before terminal `task closeout` updates the
record status. `qcold status` also triggers the refresh and prints compact
`task-record-tokens` and `task-record-efficiency` aggregates when task records
contain imported telemetry. `qcold task-record show <task-id>` prints
`token-usage`, `token-efficiency`, and top `token-efficiency-top` tool-output
samples for the selected record when that per-task telemetry is available.
`qcold task-record audit [--repo-root <path>] [--top <n>]` summarizes telemetry
coverage gaps, cost by source/outcome, and the noisiest task records by total
tokens and tool-output tokens. It is a metadata audit, not a quality score:
missing `token_efficiency`, high large-output ratios, or expensive blocked
records are surfaced as operator review targets.
When work runs on a remote task host, keep the operator machine as the canonical
Q-COLD state owner. Start new remote work with
`qcold task open-remote --via remote-dev-env <task-slug> [profile]`; this
creates the local task record first, reserves the local repo-scoped sequence,
and runs the remote repository adapter as `cargo xtask task open <task-slug>
[profile]` by default. Q-COLD passes the reserved sequence to that adapter as
`QCOLD_TASK_SEQUENCE`; it also forwards `QCOLD_TASKFLOW_DESCRIPTION`,
`QCOLD_TASKFLOW_PROMPT`, `CODEX_THREAD_ID`, and `CODEX_ROLLOUT_PATH` when
available. Repository adapters with their own environment namespace can add
aliases with repeated `--remote-task-sequence-env <name>`,
`--remote-task-description-env <name>`, `--remote-task-prompt-env <name>`,
`--remote-codex-thread-env <name>`, and `--remote-codex-rollout-env <name>`.
For example, a repository-local adapter can receive both Q-COLD's generic
sequence value and its own task-flow sequence variable in the same remote
`env` invocation. Use
`--remote-adapter <program>` plus repeated `--remote-adapter-arg <arg>` when a
remote repository exposes a different adapter command, or
`--no-default-remote-adapter-arg` when the adapter program should receive
`task ...` directly instead of the default `xtask` prefix.
Refresh local monitoring with
`qcold task-record sync-remote --via remote-dev-env --local-repo-root <local>
--remote-repo-root <remote>`. The sync runs the remote repository adapter as
`cargo xtask task export-records --limit <n>` by default and imports emitted
`task-record-json<TAB>{...}` rows into the local SQLite DB. Imported rows are
mapped to the local repository root for common numbering and dashboard
filtering, while remote paths and remote sequence values are preserved in
metadata. `qcold task-record sync-remote --legacy-remote-qcold` keeps the old
Q-COLD-to-Q-COLD replication path for hosts that have not migrated yet; new
remote task hosts should provide the repo-owned adapter export instead.
`qcold task-record export` remains the local full-record JSONL surface for
manual diagnostics and compatibility.
Read-only task-record commands keep serving the last stored records if a
concurrent dashboard or agent process temporarily blocks the telemetry refresh;
they print a warning instead of failing the read. SQLite lock waits default to
30 seconds and can be overridden for one process with
`QCOLD_SQLITE_BUSY_TIMEOUT_MS`.

## Queue CLI

The dashboard queue is also available from the standalone CLI for agents that
receive a prompt package and need to stage work without reading Q-COLD source:

```bash
qcold q-help
qcold queue run --from queue.json --agent c1 --repo-root /path/to/repo
qcold queue list
qcold queue create "client queue"
qcold queue switch <queue-tab-id>
qcold queue stop
qcold queue continue <run-id>
qcold queue delete <queue-tab-id>
```

`qcold queue run` and other mutating queue commands post to the local dashboard
daemon on `127.0.0.1:8787` by default. If the daemon is not reachable, Q-COLD
starts `qcold telegram serve --daemon` for that listen address before sending
the request. Use `--listen <addr>` to target another local dashboard daemon, or
`--no-start-daemon` to fail instead of starting one.

Queue tabs let one dashboard daemon keep separate task queues for different
repositories or workstreams. The default `Task Queue` tab is always present.
`qcold queue create` creates and activates an empty tab, `qcold queue switch`
changes which tab receives default run/append/stop/continue operations, and
`qcold queue delete` removes a non-default tab only when it has no running
queue work. The web dashboard exposes the same tabs above the queue editor.
Use the dashboard `New queue` button to create and activate another queue tab.

Prompt packages can be JSON manifests, directories, plain text files, or ZIP
archives. A JSON manifest may define shared `layers`, optional
`default_layers`, an `execution_mode` of `sequence` or `graph`, and queued
`items` with `prompt`, `slug`, and optional `depends_on` fields. Graph
`depends_on` entries may name either an item `id` or `slug`; the backend stores
them as item ids. Directory and ZIP packages use `layers/*.md` as shared prompt
layers and `prompts/*.md` or `tasks/*.md` as queued task prompts; a root
`queue.json` manifest takes precedence when present.

## Web interface

The local web dashboard server exposes the operator dashboard. It includes
task-flow status, managed agents, attachable terminal panes, and the backend
task queue for starting tracked agents.

The web dashboard terminal slash menu sends Codex Resume as `/resume --all` so
`c1`/`cc1` and `c2`/`cc2` history remains visible across Q-COLD agent worktrees.

`qcold repo add` stores repository connections in the local Q-COLD
SQLite database. Adapter-backed commands such as `status`, `task`, `verify`,
`ci`, `build`, `install`, `compat`, and `ffi` use the active repository when
the command is launched from that checkout or from one of its managed
worktrees. They fail instead of silently dispatching when the current git
checkout and resolved target repository disagree, including when
`QCOLD_REPO_ROOT`, `QCOLD_ACTIVE_REPO`, or a registry active repository points
at another checkout. Worktree-sensitive commands such as `task closeout`,
`task enter`, `task finalize`, `task iteration-notify`, and validation commands
run from a managed task worktree use that worktree even when a primary checkout
is the active repository. The web dashboard and Q-COLD-started Codex agents use
the daemon's current git checkout when the daemon was launched from one, falling
back to the active repository only when the daemon cwd is not inside a checkout.
If no repository is registered, Q-COLD falls back to
the current checkout for development compatibility. `QCOLD_REPO_ROOT` and
`QCOLD_ACTIVE_REPO` can override the resolved connection for one process only
when the override is coherent with the current checkout or managed worktree.

Telegram outbound notifications still use `TELEGRAM_BOT_TOKEN` plus
`QCOLD_TELEGRAM_OPERATOR_CHAT_ID` or `TELEGRAM_CHAT_ID` through the repository
adapter notification flow. Q-COLD-started terminal agents forward
`TELEGRAM_ENV_FILE` and `JIRA_ENV_FILE` paths, and discover a repository-local
`.env.taskflow-telegram.local` when no explicit env-file path is set, so queue
closeouts use the same repository adapter notification flow as manual task
closeouts. Keep bot tokens in the env file; Q-COLD forwards non-secret
endpoint and chat-id settings but does not inject raw token values into
terminal shell prefixes or queue task packets. Inbound Telegram control is
frozen: `qcold telegram
poll` acknowledges updates so they do not accumulate, clears the bot slash
command menu with `setMyCommands`, and deliberately does not route messages,
slash commands, Mini App launch requests, task creation, agent starts, or
chat. The web dashboard itself is served by
`qcold telegram serve --listen <addr>` and exposes an Axum-backed operator
dashboard with repository context, task-flow status, managed agents, terminals,
and the task queue. Use `--daemon` for the persistent local control plane:

```bash
qcold telegram serve --listen 127.0.0.1:8787 --daemon
```

Daemon mode forks the current Q-COLD executable, detaches it from agent
lifetimes, writes pid and log files under `QCOLD_STATE_DIR` or
`~/.local/state/qcold`, and replaces any previous Q-COLD Mini App daemon for
the same listen address. Queue task opening tolerates replacing the installed
`qcold` binary while the daemon is running by falling back to the current
`qcold` on `PATH` when the daemon's original executable path is no longer
runnable. The web assets are embedded in the Q-COLD binary, so rerun the daemon
command after `qcold install` or another Q-COLD rebuild to serve the same
binary/assets version that was just installed. Without `--daemon`,
`qcold telegram serve` stays in the foreground for systemd or other external
supervisors.

On WSL 2 with user systemd available, install a user service that starts the
dashboard automatically when the WSL user manager starts:

```bash
qcold wsl autostart install --repo-root /path/to/qcold
qcold wsl autostart status
```

The service runs `qcold telegram serve --listen 127.0.0.1:8787` in foreground
mode from the configured repository root, restarts on failure, and replaces an
older `--daemon` dashboard for the same listen address during install. Use
`--listen <addr>`, `--service-name <name>`, or `--qcold-bin <path>` when the
defaults are not right for the local WSL distribution. Use
`qcold wsl autostart remove` to disable and remove the user service. A normal
install restarts the service; `--no-start` only enables it for the next WSL
user-manager start. This configures startup inside WSL; Windows still needs to
launch the WSL distribution after a Windows reboot.

The daemon warms and maintains the dashboard state snapshot before accepting
web requests, then refreshes it in the background. Page reloads and live event
ticks read the ready snapshot instead of recomputing task records, queue state,
terminal panes, and Codex telemetry synchronously on every request. Successful
queue, terminal, and task-chat mutations request an immediate refresh so the
next live tick catches up without blocking the mutation response.

The dashboard opens to the Queue view and keeps repository/task/agent overview
state in a compact always-visible status strip. Its Queue view accepts
one task prompt at a time, appends it to a visible ordered queue, shows a
dropdown of registered repositories plus one preferred Codex-like command per
authenticated available account (`c1`, `c2`, or `codexN`) with readiness
status, and starts
one fresh Q-COLD terminal agent per queued prompt through `/agent_start --cwd
<repo>`, with internal agent track and task slug names generated automatically.
The Queue does not pre-open queued rows or choose a task profile/container from
repository policy text. It starts the selected Codex-like executor in the
repository root, sends the task slug and prompt, and tells that executor to use
the repository-native task-flow contract from `AGENTS.md` and the operator
request. If remote work, a devcontainer, full-QEMU, or another proof
environment is required, the executor opens or enters that environment itself.
`QCOLD_QUEUE_REMOTE_LAUNCHER=<launcher>` from the submitting CLI process,
`selected_remote_launcher` on a queue JSON payload, or per-item
`remote_launcher` is passed only as an `available_remote_launcher` hint for that
executor; it is not a selected profile and Q-COLD does not run
`qcold task open-remote` on the executor's behalf. Set
`QCOLD_QUEUE_REMOTE_LAUNCHER=local` or a JSON launcher value of `local` to
suppress that launcher hint.
Direct terminal agents started from a repository through Q-COLD wrappers are
also valid task entry points. Those agents run from
`../WT/<repo>/agents/<agent>` worktrees with `QCOLD_REPO_ROOT` pointing at the
primary checkout, and active-repository commands such as `qcold status` and
`qcold task open <slug>` continue to target that primary checkout.
By default, Queue execution remains ordered and starts only the first unfinished
row. Enabling Graph execution changes the draft into explicit waves from top
to bottom: Wave 1 runs first, Wave 2 waits for Wave 1, and so on. Cards inside
one wave run in parallel. `Add wave` appends a new wave below the existing
waves, and new prompts are added to the last wave by default. Task cards can be
moved by dragging them into a wave. Waves can be reordered by dragging the wave
header and removed while empty. Once a queue has been started, rows already
claimed by an executor are locked because their task packet has already been
sent, while unclaimed pending rows and later waves remain editable for the
whole non-terminal run. `Add wave` can append a later wave during an active
graph run, and new prompts added to that wave are persisted with the correct
backend dependencies. Each card has a short prompt preview, a dedicated
full-prompt action, and a `Blocks next wave` toggle that controls whether the
card blocks later waves. In Graph execution, all queued tasks whose prerequisites have
reached `closed:success` are started in parallel through separate Q-COLD
terminal agents; downstream tasks wait until their dependency set succeeds.
Each queued row starts the selected Codex-like command without an argv prompt,
waits for the attachable terminal pane, sends `/new`, and then sends the
generated managed-task instruction so the row does not inherit the previous
Codex chat context. That instruction is a compact `Q-COLD_TASK_PACKET` with
the repository root, task slug, selected command, required task-flow commands,
validation and closeout expectation, blocker boundary, state pointers for the
eventual `.task/task.env` and task logs, an output guard policy, a bounded
operator-request snippet, and the full operator request for the executor.
If the Codex CLI presents its interactive update menu during queue launch,
the backend accepts the default update action. When Codex exits after a
successful self-update and asks to restart, the backend removes that stale
executor record and retries the launch immediately before sending the task
packet.
Queue launcher agents use slug/repository-derived display labels and short
session ids rather than prompt-derived labels. They are internal transport and do not create
separate ad-hoc task records; the visible task state belongs to the managed
`task/<slug>` record.
The Queue is run by the Mini App backend, not by a long-lived browser loop.
The browser submits the queued rows to `/api/queue/run`, can append more rows
to that active run through `/api/queue/append`, and otherwise only renders the
backend queue snapshot. The backend stores the active run in Q-COLD state,
starts one fresh Q-COLD terminal agent per runnable queued prompt, waits for the
matching managed `task/<slug>` record to reach `closed:success`, and then
advances any newly unblocked graph nodes. After a row reaches
`closed:success`, Q-COLD terminates the row's executor agent terminal while
keeping the completed queue row as run
history; for Zellij-backed agents, cleanup deletes the session record instead
of leaving a resurrectable exited session in the terminal list. If the backend
is restarted while a queue run is active, the next snapshot reconciles already
closed task records even if a queue row still has an older launch repo path,
cleans up stale queue-agent terminals, and resumes the queue worker from
persisted run state before reporting the run state. If the selected agent account is temporarily
unavailable, the
backend waits and retries the next launch three times after roughly 1, 5, and
10 minutes before failing the row; unauthenticated accounts fail immediately.
Executor launcher setup failures before a managed task record exists are
retryable on the same schedule. Repository task-open, remote transport, and
environment-bootstrap failures happen inside the executor-owned task flow and
must be surfaced through the matching `task/<slug>` record.
Once the matching `task/<slug>` record exists,
Q-COLD will not start a second executor for that row; non-success closeout or a
prematurely exited executor stops the row for operator diagnostics. If the
operator later resumes a blocked task chat and that managed task reaches
`closed:success`, stale queue reconciliation promotes the stopped row to
success and resumes any now-unblocked later graph waves.
The Queue Stop action stops the backend worker immediately and marks the
current row as stopped without deleting it or treating it as complete. The same
control becomes Continue for a stopped run; continuing clears the stop flag and
resumes from that row, reusing its still-running executor agent when one exists
or launching a fresh executor when needed.
Draft queue rows can be reordered, removed, copied, cleared in bulk, or opened
to an interactive task chat. Once a queue has been started, row order is owned
by the backend run for active rows; completed rows and not-yet-started pending
rows remain removable while the queue is running, and pending graph rows can
still update their prompt text, wave placement, and dependency gates. Pending
ordered rows can also be moved among other pending rows after the active
cursor.
Bulk clearing removes persisted queue rows, removes the
matching task records, and terminates any associated executor agents. When the
related terminal agent is still running, that chat can send
operator messages back into the pane even before Codex telemetry has captured a
session transcript; if no transcript is available yet, the modal falls back to
the live terminal output. Blocked task chats remain operator-actionable: if the
original pane has exited but the saved Codex session id is known, Q-COLD starts a
fresh attachable `resume` terminal target, applies the task slug/repository
display label, and sends a `Q-COLD_RESUME_PACKET` that references only visible
task state paths that exist before sending the next operator message. Removing
a persisted queue row is the cleanup boundary for that work:
the backend removes the row, removes the matching `task/<slug>` record, and
terminates the associated executor agent when one is still known. Rows without a
task record still switch to the Tasks view while recording a row-level
availability note. Any blocked, failed, unknown, or prematurely exited task stops
the remaining queue until a later resumed task-flow record reaches
`closed:success`. Queue draft rows may still use browser local storage before
launch, but live queue state, retry counters, agent ids, and generated
`task/<slug>` values come from the backend snapshot after launch, so refreshing
the tab does not stop the active run. Queue rows, task cards, agent cards, and
terminal cards share the same terminal agent display name when one is known, with
short technical ids kept as secondary diagnostics. Queue executor terminals use
their terminal scope for the managed `task/<slug>` id, so the Terminals view
keeps both the agent name and the task anchor visible. Open queue tasks route
only to the known executor terminal; saved Codex transcripts are exposed for
queue tasks after terminal closeout, not while the row is still live. When
Codex telemetry has captured a session path, task records expose the saved chat
transcript from the Tasks view even after the terminal agent has exited. Q-COLD assigns Codex
sessions to managed tasks only through the session id plus structured
`session_meta.cwd` or tool-call `workdir`/`cwd` fields under the managed task
worktree; arbitrary task-id text in prompts or tool output is not enough to
claim a task transcript. Its
Tasks view shows Q-COLD task records for the active repository from SQLite as
separate active and historical sections, including open/closed counts, last-24-hour activity,
aggregate Codex token telemetry, and average closed-task token cost imported
from session JSONL metadata. Long task descriptions are collapsed into a
single-line preview with a prompt disclosure so task cards stay scannable while
the full prompt remains available in place. Raw managed-worktree status remains
available for debugging, but terminal readiness ignores task worktrees whose
task env has already reached a `closed:*` status. It streams state updates with
server-sent events and includes an `Auto`/`Dark`/`Light` theme switch stored in
browser local storage. The Agents view shows
detected local agent commands only when their account `auth.json` exists, then
shows account/readiness probe status before the running-process sections.
Readiness probes run through a cached
`/api/agent-limits` request when the Agents view is opened or refreshed. Q-COLD
uses each account's base command such as `c1`, `c2`, or `codexN` with
`--version`, retries transient failures, and avoids compact `cc*` wrappers so
probing does not create model sessions. The bare `codex` executable remains a
supported explicit `agent start` command, but it is not advertised as an
available account. The same view shows only
currently running Q-COLD tracked agents and separates them from host-discovered
agent programs: native `codex` processes plus the Q-COLD web control daemon.
Exited Q-COLD agent records remain available through the CLI registry surface,
but the dashboard omits them as historical noise. Task-flow helper programs
such as `xtask` are not counted as agents.
Q-COLD prunes stale agent registry noise before agent snapshots: exited agent
rows and stale terminal agents older than `QCOLD_AGENT_STALE_TTL_HOURS`
(default `2`) are removed with their ad-hoc `agent`/`codex-session` task
records. Running terminal agents are terminated only when their tmux/zellij
session has no attached clients. Run `qcold agent prune-stale --dry-run` to
inspect the same cleanup, or pass `--include-attached` for an explicit manual
cleanup that also closes old attached terminal sessions.
The Terminals view exposes attachable terminal panes for agent programs,
captures the recent pane scrollback with ANSI color/style attributes, sends
prompt composer text through backend-native paste plus a submit key, and can
forward focused terminal-output keystrokes as terminal input. Browser snapshots
keep roughly the last 2,000 terminal lines, while new Q-COLD tmux sessions keep
a deeper local scrollback for future snapshots. Output refreshes keep following
the terminal tail only while the pane is already scrolled near the bottom; if
the operator scrolls up to read history, new output preserves that reading
position. Single-line slash
commands are sent as literal key input instead of bracketed paste so Codex TUI
slash commands can open normally, and terminal composers show a filtered slash
command menu when the draft starts with `/`. Empty composer history arrows are
forwarded to the underlying pane. The view gives Q-COLD-started terminals short
generated Greek philosopher names
such as `Socrates` or `Diogenes` and keeps them unique among running agents.
Existing discoverable terminals fall back to their session and current
command. Click a terminal title in the browser to override its name or set a
short scope label such as `refactoring`; those overrides are stored in Q-COLD
state. The default backend is `tmux`.
Set `QCOLD_TERMINAL_BACKEND=zellij` to start new Q-COLD terminal agents through
`zellij` instead; the GUI discovers both Q-COLD `tmux` and `zellij` sessions.
With the zellij backend, `qcold agent start --terminal --name "<pane name>" ...`
sets the zellij pane title and the Q-COLD terminal display name. It does not
send Codex TUI `/rename`. Q-COLD also emits a host terminal title escape while
attaching, so terminal tabs show the short display name instead of the wrapper
command. If `--attach --name "<pane name>"` matches one running same-track
Q-COLD terminal display name, Q-COLD attaches to that terminal instead of
starting a duplicate session. A later plain named Codex launch, such as
`cc1 --name atomic`, resumes the latest exited same-track named Codex chat when
Q-COLD has imported that prior session id; otherwise it starts a fresh chat.
When the newer same-name terminal exited cleanly, such as through Codex `/quit`,
Q-COLD treats that name as intentionally closed and starts fresh instead of
falling back to older interrupted sessions; the closed named-session record and
local terminal logs remain available until explicitly dropped. Use
`qcold agent named-sessions list --agent cc1` to inspect Q-COLD's named Codex
sessions, `qcold agent named-sessions drop --agent cc1 --name atomic`
to drop one stale name, or `qcold agent named-sessions drop-all --agent cc1`
to clear all named sessions for that agent account and track. These commands
remove Q-COLD's resume binding records and local terminal logs, but they do not
delete raw Codex session JSONL transcripts. Running terminals are skipped
unless `--include-running` is passed.
Plain processes started in a non-multiplexed console are visible as host
processes but are not safely attachable after the fact. Start agents with
`qcold agent start --terminal --attach --track <track> -- <command>...` to see
the same session in the local terminal and in the Q-COLD Terminals view. For a
local `c2` wrapper, use the command shape
`qcold agent start --terminal --attach --track c2 -- c2 "<prompt>"`.
For an agent that was started from the web queue, run
`qcold agent list` to see its generated name and target, then attach from a
local terminal with `qcold agent attach <agent-id|target|session|name>`.
Q-COLD starts Codex-like agent commands (`c1`, `cc1`, `c2`, `cc2`, `codex`,
and `codexN`) from an explicit launch directory instead of inheriting the
daemon cwd. If the launch directory is not already a managed task worktree,
Q-COLD first creates a persistent agent-owned Git worktree under
`../WT/<repo>/agents/<agent-id>/`, initializes Git submodules in a two-phase
bootstrap that seeds local primary-checkout submodule caches before the
recursive update when `.gitmodules` is present, and
then starts the agent from that worktree. This keeps Codex resume and
context-compaction fallbacks anchored in the agent's isolated workspace instead
of the primary checkout. The agent-owned worktree is a host-side home base, not
a task devcontainer; the agent should enter a devcontainer only after opening a
specific managed task worktree. Task worktrees opened later by the agent remain
separate and can be closed without deleting the agent workspace. For
interactive Codex launches without an explicit `--cwd`, Q-COLD reuses the
latest compatible same-track exited agent worktree for the repository only when
that worktree still points at the current checkout HEAD and matching branch
identity when both worktrees are on named branches. Explicit `resume` launches
can also reuse the latest compatible worktree, so normal `cc1`/`cc2`
restarts come back in the same cwd and Codex's in-chat `/resume` picker sees
sessions and metadata saved from previous runs instead of starting from a fresh
empty agent cwd. Launches started from inside an existing agent-owned worktree
stay in that worktree instead of opening or reusing another one. Q-COLD exports
the primary checkout as `QCOLD_REPO_ROOT` and the agent-owned worktree as
`QCOLD_AGENT_WORKTREE` for the launched agent, so active inventory commands such
as `qcold task list` resolve through the task's primary checkout.
Worktree-sensitive commands such as `task closeout` still prefer the current
managed task worktree when the agent has changed into one. In that agent
context, successful task closeout leaves the closed task worktree detached and
untracked instead of removing the directory from under the live agent process;
the closeout output prints `task-return <agent-worktree>` so the agent can
return to its stable workspace before starting another chat or task. Use
`--cwd <path>` to choose the launch context explicitly. Set
`QCOLD_AGENT_MANAGED_WORKTREE=0` only for debugging when this automatic
isolation should be bypassed.
When the wrapped agent exits, the terminal session exits too, so `/quit` in an
attached agent returns to the parent terminal without an extra shell prompt.
That clean exit also prevents the same terminal name from auto-resuming the
closed Codex chat on the next plain named launch.
Terminal backend follow-up work is tracked in
[`docs/terminal-backlog.md`](docs/terminal-backlog.md).
GUI command execution is enabled for the local server by default. If the GUI is
intentionally exposed beyond the local host, set
`QCOLD_WEBAPP_REQUIRE_WRITE_TOKEN=yes` and `QCOLD_WEBAPP_WRITE_TOKEN`; POST
requests must then send it as `X-QCOLD-Write-Token`.

`qcold guard -- <command>...` runs a local command and suppresses stdout/stderr
when the combined output is too large. Use it before risky broad searches,
large log reads, or repository-wide reports. The guard stops reading after the
limit is crossed instead of retaining the full raw output. The default limits
are 16 KiB and 400 lines; when either limit is exceeded, Q-COLD prints a compact blocked
message and exits with code 2 so the operator or agent can rerun a narrower
query such as `rg -l`, `rg --count`, `sed -n`, `head`, `tail`, or a path-limited
search.

Q-COLD-managed agent and task contexts also get automatic guard wrappers for
broad output commands. By default, each Q-COLD agent launch prepends a
per-context `QCOLD_OUTPUT_GUARD_BIN` directory to `PATH` with wrappers for
`rg`, `grep`, `find`, `cat`, `git`, `unzip`, `zcat`, and `jq`; each wrapper
invokes `qcold guard -- <real-command> "$@"` using the real command path
resolved before the guard directory is added. Q-COLD also records compact guard
provenance in managed task `.task/task.env`, including
`QCOLD_OUTPUT_GUARD_ENABLED`, `QCOLD_OUTPUT_GUARD_BIN`, and
`QCOLD_OUTPUT_GUARD_COMMANDS`. `qcold task enter` prints matching shell exports
when the task env has guard metadata, so Q-COLD-controlled task handoffs can
restore the guarded `PATH`.

Set `QCOLD_AGENT_OUTPUT_GUARD=0` to disable wrapper setup for one launch or task
open. Set `QCOLD_AGENT_OUTPUT_GUARD_COMMANDS=rg,grep,find,cat,git,unzip,zcat,jq`
to override the command list. `sed`, `awk`, and `ls` are intentionally opt-in:
they are common shell-script data plumbing and wrapping them by default is more
likely to change normal validation or build helper behavior. For bundle,
performance, or broad-audit work where a stricter interactive shell is useful,
set `QCOLD_AGENT_OUTPUT_GUARD_COMMANDS=rg,grep,find,cat,git,unzip,zcat,jq,sed,awk,ls`.
Invalid command names are rejected; use bare command names, not paths or shell
fragments.

Automatic guard setup applies to `qcold agent start`, web/queue-started agents,
managed task worktree agent sessions, and Q-COLD-controlled task-open/task-enter
handoffs. Adapter-owned validation and closeout subprocesses scrub inherited
guard `PATH` state so repository automation is not silently changed. Guard
wrappers cannot intercept absolute command paths, shell builtins, aliases,
already-running external terminals, or non-Q-COLD-launched processes.

The self-hosted `xtask` adapter also keeps internal machine-readable Git
transport separate from terminal output: its internal Git helper uses recorded
`QCOLD_GUARD_REAL_*_GIT` paths when present and strips inherited guard wrappers
from subprocess environments. Broad internal data such as `git ls-files -z` is
consumed by the adapter; only the adapter's final operator-facing report is
expected to pass through the output guard.

Q-COLD state is stored in one local SQLite database under `QCOLD_STATE_DIR` or
`~/.local/state/qcold/qcold.sqlite3`. The database owns repository registry,
managed-agent records, Telegram task topics, events, and the
initial schema for runs, claims, budgets, and recipes. Legacy `agents.tsv`,
`telegram_tasks.tsv`, and `task-events/*.log` files are imported on first read
when the corresponding SQLite tables are empty.

Telegram Mini App launch and inbound chat routing are currently suspended.
`QCOLD_TELEGRAM_WEBAPP_URL`, `/app`, `/task`, plain Telegram messages, and
Telegram replies are ignored by the poller while the Telegram control plane is
frozen.

Historically, in a forum supergroup, `/task <description>` created a per-task
topic when the bot had permission to manage topics. Existing topic mappings
remain stored under `QCOLD_STATE_DIR` or `~/.local/state/qcold`, but new
messages in those topics are ignored while inbound Telegram control is frozen.

Adapter-backed commands run from a target repository checkout with
`cargo xtask`, or through `QCOLD_XTASK_MANIFEST` when an explicit xtask
manifest path is needed. In the Q-COLD checkout itself, `.cargo/config.toml`
defines `cargo xtask` as the self-hosted adapter in `xtask/`, so normal
development can start with `QCOLD_REPO_ROOT=$PWD qcold task open <slug>`
from a clean primary checkout, or with plain `qcold task open <slug>` when
the Q-COLD repository is the active registered repo. Q-COLD self-hosted task
opens default `TASK_PROFILE=e2e`; pass `slim` explicitly only for lightweight
work that should avoid the E2E-capable environment. The Q-COLD self-hosted
adapter requires new task opens to start from `main`; it fails before creating
a task worktree when the primary checkout is on another branch. Registered
repositories may set `--default-branch <branch>`, which Q-COLD forwards to the
process adapter as `QCOLD_TASK_OPEN_BASE_BRANCH`; repository adapters may also
set `taskflow.base-branch` in git config. `qcold task pause --reason "<reason>"`
records a non-terminal pause for work that needs operator
input or an external unblock while preserving the managed worktree for direct
resume. Technical validation blockers that are small, mechanical, and within
task scope should be fixed by the agent in the same task state instead of being
paused or closed as blocked. Self-hosted `task terminal-check` and
`task orphan-clear-stale` clean paused task state older than
`QCOLD_PAUSED_TASK_TTL_HOURS`, defaulting to 2 hours. ZIP bundles under
`bundles/` are retained separately for `QCOLD_BUNDLE_RETENTION_HOURS`,
defaulting to 24 hours. A sibling checkout layout is still supported as a local
convenience for development:

```text
repos/github/
  qcold/
  target-repo/
```

The production dependency graph does not include `../target-repo/xtask`; adapter
calls cross the process boundary.

`qcold bundle` writes one source ZIP archive for the current repository into
the repository-local `bundles/` directory, which is ignored by git. The command
requires a clean worktree and prints `BUNDLE_PATH=...` for handoff. Bundle
metadata is embedded inside the ZIP at `metadata/bundle-manifest.txt`, and
root `summary.md` provides the human-readable handoff. The self-hosted task
cleanup path removes stale ZIP bundles only after the bundle retention window,
which defaults to 24 hours.

## Development contract

This repository follows the task-flow and delegation discipline captured in
[`AGENTS.md`](AGENTS.md). Q-COLD owns a minimal self-hosted task-flow adapter
for dogfooding: managed worktrees are created under `../WT/qcold/`, success
closeout runs `cargo fmt --check` plus the serial `cargo-qcold` unit suite,
then runs a mandatory pre-merge quality review before delivery. The reviewer
must return `REVIEW_STATUS=pass` or `REVIEW_STATUS=block` plus argued
criticism, a `REVIEW_SUMMARY=...` line, and at least one argued finding
bullet. Missing, failed, vacuous, or blocking review output stops success
closeout before merge/push. Set `QCOLD_CLOSEOUT_REVIEWER_COMMAND` to an
injectable reviewer command for one process. Q-COLD passes
`QCOLD_REVIEW_PROMPT` and `QCOLD_REVIEW_OUTPUT`; the command should read the
prompt and write the final report to the output path. Without that override,
the self-hosted adapter uses `c1 exec` in read-only mode for the review.
Success closeout prints `task-closeout-phase` `start`/`ok` rows and
`task-closeout-review` `started`/`finished` rows so long review waits are
visible before the final terminal receipt. After a passing review, closeout
fast-forwards the primary checkout to the current
remote base, rebases the task branch onto that base, pushes the base branch to
`origin`, and refreshes the remote-tracking ref before terminal cleanup.
Terminal task bundles are self-contained ZIP archives. They include root
`summary.md` for human handoff, `metadata/bundle.env`,
`metadata/terminal-receipt.env`, promoted task logs under `logs/`, focused git
evidence under `evidence/`, the pre-merge review report at
`evidence/pre-merge-review.md`, structured review metadata at
`metadata/pre-merge-review.env`, reviewer prompt and command diagnostics under
`evidence/`, and a git-visible repo snapshot under `repo/`.
Generated local output families such as `target/`, `build/`, `dist/`,
`node_modules/`, and `bundles/` are excluded from that snapshot. If success
closeout fails after task state is available, the adapter records
`STATUS=failed-closeout`, preserves the task worktree, and writes the same
diagnostic bundle shape with `CURRENT_FLOW_PROBLEM`,
`HISTORICAL_FLOW_PROBLEM`, `CLOSEOUT_FAILURE_PHASE`, and
`CLOSEOUT_FAILURE_ERROR` fields in `metadata/terminal-receipt.env`.
Repository-specific proof semantics for other projects remain behind their adapters.
Adapters that run E2E or compatibility proof lanes should keep raw logs in
bundles or task-local logs, but commit only the compact recent result index.
The reference path is `compat/evidence/proof-runs.tsv`; it has no bundle path,
bundle filename, or bundle hash column, includes `task_sequence`, `task_id`,
and `task_name` when available, and retains only the latest 20 data rows. The
Q-COLD self-hosted adapter treats `.task/logs/compat/**/summary.tsv` files with
positive proof counters as input to that index during success closeout. Target
repository adapters own their own extraction details and must not present those
repository-specific proof rules as Q-COLD facade behavior.
The planned extraction backlog for moving deterministic task-flow ownership
into Q-COLD is tracked in
[`docs/taskflow-extraction/`](docs/taskflow-extraction/README.md).
