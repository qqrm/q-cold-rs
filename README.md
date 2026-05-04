# Q-COLD

Q-COLD is an extracted orchestration facade for agent-driven task-flow work.
The name expands to QQRM Collaboration, Orchestration, Lifecycle, and Delivery.

![Q-COLD web dashboard terminals](docs/screenshots/qcold-web-terminals.jpg)

Q-COLD owns the operator-facing command surface and keeps repository-specific
proof, validation, and closeout semantics behind explicit adapter traits. The
first adapter is a generic xtask process adapter, so Q-COLD can build without
a Cargo path dependency on any target repository. Install the standalone
operator binary plus Cargo subcommand compatibility locally with:

```bash
cargo install --path . --locked
```

Then register repository connections in Q-COLD state and run adapter-backed
commands through the active repository:

```bash
qcold repo add target-repo /path/to/target-repo \
  --xtask-manifest /path/to/target-repo/xtask/Cargo.toml \
  --set-active
qcold repo list
qcold status
qcold agent list
qcold agent start --track audit -- codex exec "inspect repo"
qcold agent start --terminal --attach --track c2 -- c2 "work on the active task"
qcold telegram poll
qcold telegram serve --listen 127.0.0.1:8787 --daemon
qcold bundle
qcold task inspect runtime-audit
qcold task open my-task
```

`cargo qcold <command>` remains supported for Cargo subcommand compatibility,
but `qcold <command>` is the primary operator interface.

## Web interface

The Telegram Mini App server exposes the local operator dashboard. It includes
the meta-agent chat, task-flow status, managed agents, attachable terminal
panes, and a command composer for starting tracked agents.

`qcold repo add` stores repository connections in the local Q-COLD
SQLite database. Adapter-backed commands such as `status`, `task`, `verify`,
`ci`, `build`, `install`, `compat`, and `ffi` use the active repository instead
of the daemon process cwd. If no repository is registered, Q-COLD falls back to
the current checkout for development compatibility. `QCOLD_ACTIVE_REPO` and
`QCOLD_REPO_ROOT` can override the active connection for one process.

Telegram polling is configured with `TELEGRAM_BOT_TOKEN` plus
`QCOLD_TELEGRAM_OPERATOR_CHAT_ID` or `TELEGRAM_CHAT_ID`. Command-capable
polling fails closed unless `QCOLD_TELEGRAM_ALLOWED_USER_IDS` or
`QCOLD_TELEGRAM_ALLOWED_USERNAMES` is set. Prefer
`QCOLD_TELEGRAM_ALLOWED_USER_IDS` with comma-separated numeric Telegram user
ids; usernames are useful only as a bootstrap fallback because they can change.
Use `/whoami` in the operator chat to see the numeric Telegram user id to place
in that allowlist. On poller startup Q-COLD publishes the current Bot API
command list with `setMyCommands` so Telegram clients can show slash-command
hints. Direct private chats are accepted only when the Telegram account is in
the operator allowlist; group and forum commands must come from the configured
chat. `/status` returns a human-readable repository task summary for Telegram,
`/agents` returns the Q-COLD managed-agent registry, and
`/agent_start <track> :: <command>` starts an agent through Q-COLD. `/repos`
shows registered repository connections and the active repository. `/app`
returns a Telegram Mini App launch button when `QCOLD_TELEGRAM_WEBAPP_URL` is
set. The Mini App itself is served by `qcold telegram serve --listen <addr>` and
exposes an Axum-backed operator dashboard with repository context, task-flow
status, managed agents, shared local history, and a meta-agent command
composer. Use `--daemon` for the persistent local control plane:

```bash
qcold telegram serve --listen 127.0.0.1:8787 --daemon
```

Daemon mode forks the current Q-COLD executable, detaches it from agent
lifetimes, writes pid and log files under `QCOLD_STATE_DIR` or
`~/.local/state/qcold`, and replaces any previous Q-COLD Mini App daemon for
the same listen address. The web assets are embedded in the Q-COLD binary, so
rerun the daemon command after `cargo install --path . --locked` or another
Q-COLD rebuild to serve the same binary/assets version that was just installed.
Without `--daemon`, `telegram serve` stays in the foreground for systemd or
other external supervisors.

The dashboard opens to the meta-agent chat and keeps repository/task/agent
overview state in a compact always-visible status strip. It streams state and
history updates with server-sent events and includes an `Auto`/`Dark`/`Light`
theme switch stored in browser local storage.
The web chat displays web-origin messages only, while the meta-agent prompt can
still use the broader shared local history as context. The Agents view
separates running Q-COLD tracked agents from host-discovered agent programs:
native `codex` processes plus the Q-COLD web control daemon. Task-flow helper
programs such as `xtask` are not counted as agents.
The Terminals view exposes attachable terminal panes for agent programs,
captures recent pane output with ANSI color/style attributes, and sends input
from each terminal card through backend-native paste plus a submit key. The
view gives Q-COLD-started terminals generated human-readable labels from the
agent track and wrapped command. Existing discoverable terminals fall back to
their session and current command. Click a terminal title in the browser to
override its name or set a short scope label such as `refactoring`; those
overrides are stored in Q-COLD state. The default backend is `tmux`.
Set `QCOLD_TERMINAL_BACKEND=zellij` to start new Q-COLD terminal agents through
`zellij` instead; the GUI discovers both Q-COLD `tmux` and `zellij` sessions.
Plain processes started in a non-multiplexed console are visible as host
processes but are not safely attachable after the fact. Start agents with
`qcold agent start --terminal --attach --track <track> -- <command>...` to see
the same session in the local terminal and in the Q-COLD Terminals view. For a
local `c2` wrapper, use the command shape
`qcold agent start --terminal --attach --track c2 -- c2 "<prompt>"`.
When the wrapped agent exits, the terminal session exits too, so `/q` in an
attached agent returns to the parent terminal without an extra shell prompt.
Terminal backend follow-up work is tracked in
[`docs/terminal-backlog.md`](docs/terminal-backlog.md).
GUI command execution is enabled for the local server by default. If the GUI is
intentionally exposed beyond the local host, set
`QCOLD_WEBAPP_REQUIRE_WRITE_TOKEN=yes` and `QCOLD_WEBAPP_WRITE_TOKEN`; POST
requests must then send it as `X-QCOLD-Write-Token`.

Q-COLD state is stored in one local SQLite database under `QCOLD_STATE_DIR` or
`~/.local/state/qcold/qcold.sqlite3`. The database owns repository registry,
chat history, managed-agent records, Telegram task topics, events, and the
initial schema for runs, claims, budgets, and recipes. Legacy `agents.tsv`,
`telegram_tasks.tsv`, and `task-events/*.log` files are imported on first read
when the corresponding SQLite tables are empty.

Telegram
Mini Apps require a public HTTPS URL; run the local server behind an HTTPS
reverse proxy or tunnel before setting `QCOLD_TELEGRAM_WEBAPP_URL`. Plain
messages in `QCOLD_TELEGRAM_META_CHAT_ID`, or replies in an allowed chat, are
routed to `QCOLD_META_AGENT_COMMAND` when it is set. If it is unset, Q-COLD
uses `codex exec --ephemeral --cd <repo> -` from the active repository, so
Codex session state is not persisted between meta-agent runs. The
meta-agent prompt includes the latest shared local history entries plus the
current operator message.

In a forum supergroup, `/task <description>` creates a per-task topic when the
bot has permission to manage topics. Q-COLD stores the topic mapping under
`QCOLD_STATE_DIR` or `~/.local/state/qcold`, and later messages in that topic
are recorded as task input.

Adapter-backed commands run from a target repository checkout with
`cargo xtask`, or through `QCOLD_XTASK_MANIFEST` when an explicit xtask
manifest path is needed. A sibling checkout layout is still supported as a
local convenience for development:

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
metadata is embedded inside the ZIP at `metadata/bundle-manifest.txt`.

## Development contract

This repository follows the task-flow and delegation discipline captured in
[`AGENTS.md`](AGENTS.md). During incubation, Q-COLD command development is
validated with local Cargo checks, while adapter-backed task-flow closeout is
used only when the required remote and devcontainer prerequisites are present.
The planned extraction backlog for moving deterministic task-flow ownership
into Q-COLD is tracked in
[`docs/taskflow-extraction/`](docs/taskflow-extraction/README.md).
