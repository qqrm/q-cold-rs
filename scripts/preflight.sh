#!/usr/bin/env bash
set -Eeuo pipefail

usage() {
  cat <<'USAGE'
usage: scripts/preflight.sh [fast|full|task-flow] [--full] [--task-flow]

Runs the local iteration gate used by Q-COLD managed closeout and CI.

  --full       also run `cargo test --locked`
  --task-flow  also run task-flow fixture suites
USAGE
}

full=0
task_flow=0
while (($#)); do
  case "$1" in
    fast|default)
      ;;
    full)
      full=1
      ;;
    task-flow)
      task_flow=1
      ;;
    --full)
      full=1
      ;;
    --task-flow)
      task_flow=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown preflight argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run cargo fmt --check

if ! command -v node >/dev/null 2>&1; then
  echo "node is required for web asset syntax checks" >&2
  exit 1
fi
run node --check src/webapp_assets/app.js

run cargo test --locked --bins
run cargo test --locked --test command_version
run cargo test --locked --test agent_repo_context
run cargo test --locked --test task_record_sequence

if ((full)); then
  run cargo test --locked
fi

if ((task_flow)); then
  run cargo test --locked --test task_flow_control_plane
  run cargo test --locked --test task_flow_regression
fi
