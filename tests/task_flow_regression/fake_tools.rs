use std::path::Path;

pub fn write_fake_docker(path: &Path) {
    super::write_exe(
        path,
        r#"#!/usr/bin/env bash
set -euo pipefail
state=${FAKE_DOCKER_STATE:?}
images=${FAKE_DOCKER_IMAGES:?}
log=${FAKE_DEVCONTAINER_LOG:-/dev/null}
container_bin=${FAKE_DEVCONTAINER_CONTAINER_BIN:-}
cmd=${1:?}
shift
case "$cmd" in
  ps)
    format=
    workspace=
    primary=
    task=
    dev_id=
    while [[ $# -gt 0 ]]; do
      case "$1" in
        -a|-q|-aq) shift ;;
        --filter)
          case "$2" in
            label=devcontainer.local_folder=*) workspace=${2#label=devcontainer.local_folder=} ;;
            label=taskflow.primary_repo=*) primary=${2#label=taskflow.primary_repo=} ;;
            label=taskflow.task_id=*) task=${2#label=taskflow.task_id=} ;;
            label=taskflow.devcontainer_id=*) dev_id=${2#label=taskflow.devcontainer_id=} ;;
          esac
          shift 2
          ;;
        --format) format=$2; shift 2 ;;
        *) shift ;;
      esac
    done
    [[ -f "$state" ]] || exit 0
    while IFS='|' read -r id name item_workspace item_primary item_task item_dev_id image item_taskflow item_legacy; do
      [[ -n "$id" ]] || continue
      [[ -n "$workspace" && ! ( "$item_legacy" == 1 && "$item_workspace" == "$workspace" ) ]] && continue
      [[ -n "$primary" && ! ( "$item_taskflow" == 1 && "$item_primary" == "$primary" ) ]] && continue
      [[ -n "$task" && ! ( "$item_taskflow" == 1 && "$item_task" == "$task" ) ]] && continue
      [[ -n "$dev_id" && ! ( "$item_taskflow" == 1 && "$item_dev_id" == "$dev_id" ) ]] && continue
      case "$format" in
        "{{.ID}}") printf '%s\n' "$id" ;;
        *) printf '%s|%s|%s|%s|%s|%s|%s|%s|%s\n' "$id" "$name" "$item_workspace" "$item_primary" "$item_task" "$item_dev_id" "$image" "$item_taskflow" "$item_legacy" ;;
      esac
    done <"$state"
    ;;
  rm)
    [[ "$1" == "-f" ]] || exit 2
    shift
    touch "$state"
    tmp="$state.tmp"
    : >"$tmp"
    while IFS='|' read -r id name item_workspace item_primary item_task item_dev_id image item_taskflow item_legacy; do
      keep=1
      for arg in "$@"; do
        [[ "$id" == "$arg" ]] && keep=0
      done
      if [[ "$keep" -eq 1 ]]; then
        printf '%s|%s|%s|%s|%s|%s|%s|%s|%s\n' \
          "$id" "$name" "$item_workspace" "$item_primary" "$item_task" \
          "$item_dev_id" "$image" "$item_taskflow" "$item_legacy" >>"$tmp"
      fi
    done <"$state"
    mv "$tmp" "$state"
    ;;
  exec)
    workspace=
    env_entries=()
    while [[ $# -gt 0 ]]; do
      case "$1" in
        -u) shift 2 ;;
        -w) workspace=$2; shift 2 ;;
        -e) env_entries+=("$2"); shift 2 ;;
        -i|-t) shift ;;
        *) break ;;
      esac
    done
    target=${1:?}
    shift
    [[ -n "$workspace" ]] || exit 2
    printf 'docker-exec|%s|%s|%s\n' "$target" "$workspace" "$*" >>"$log"
    cd "$workspace"
    unset QCOLD_TASKFLOW_CONTAINER_ROOT QCOLD_TASKFLOW_CONTEXT \
      QCOLD_TASKFLOW_PRIMARY_REPO_PATH QCOLD_TASKFLOW_TASK_ID \
      QCOLD_TASKFLOW_TASK_WORKTREE QCOLD_TASKFLOW_TASK_BRANCH \
      QCOLD_TASKFLOW_DEVCONTAINER_ID CARGO_TARGET_DIR
    for entry in "${env_entries[@]}"; do
      export "$entry"
    done
    if [[ -n "$container_bin" ]]; then
      PATH="$container_bin:$PATH" "$@"
    else
      "$@"
    fi
    ;;
  inspect)
    inspect_type=container
    if [[ "${1:-}" == "--type" ]]; then inspect_type=${2:?}; shift 2; fi
    target=${1:?}
    format=
    if [[ "${2:-}" == "--format" ]]; then format=${3:?}; fi
    if [[ "$inspect_type" == image ]]; then
      [[ -f "$images" ]] || exit 1
      while IFS='|' read -r repo tag image_id image_primary; do
        [[ -n "$repo" ]] || continue
        ref="$repo"
        [[ -n "$tag" && "$tag" != "<none>" ]] && ref="$repo:$tag"
        [[ "$target" == "$ref" || "$target" == "$repo" || "$target" == "$image_id" ]] || continue
        printf '{"devcontainer.metadata":"{\"mounts\":[\"source=${localWorkspaceFolder},target=%s,type=bind,consistency=cached\"]}"}\n' "$image_primary"
        exit 0
      done <"$images"
      exit 1
    fi
    [[ -f "$state" ]] || exit 1
    while IFS='|' read -r id name item_workspace item_primary item_task item_dev_id image item_taskflow item_legacy; do
      [[ -n "$id" ]] || continue
      [[ "$target" == "$id" || "$target" == "$name" || "$target" == "/$name" ]] || continue
      case "$format" in
        "{{.Name}}") printf '/%s\n' "$name" ;;
        "{{.Config.Image}}") printf '%s\n' "$image" ;;
        "{{json .Config.Labels}}") printf '{"taskflow.primary_repo":"%s","taskflow.task_id":"%s","taskflow.devcontainer_id":"%s"}\n' "$item_primary" "$item_task" "$item_dev_id" ;;
        *) exit 2 ;;
      esac
      exit 0
    done <"$state"
    exit 1
    ;;
  images)
    format=
    while [[ $# -gt 0 ]]; do
      case "$1" in --format) format=$2; shift 2 ;; *) shift ;; esac
    done
    [[ -f "$images" ]] || exit 0
    while IFS='|' read -r repo tag image_id image_primary; do
      [[ -n "$repo" ]] || continue
      case "$format" in
        "{{.Repository}}|{{.Tag}}") printf '%s|%s\n' "$repo" "$tag" ;;
        *) printf '%s|%s|%s|%s\n' "$repo" "$tag" "$image_id" "$image_primary" ;;
      esac
    done <"$images"
    ;;
  rmi)
    [[ "$1" == "-f" ]] || exit 2
    shift
    touch "$images"
    tmp="$images.tmp"
    : >"$tmp"
    while IFS='|' read -r repo tag image_id image_primary; do
      ref="$repo"
      [[ -n "$tag" && "$tag" != "<none>" ]] && ref="$repo:$tag"
      keep=1
      for arg in "$@"; do
        [[ "$arg" == "$repo" || "$arg" == "$ref" || "$arg" == "$image_id" ]] && keep=0
      done
      [[ "$keep" -eq 1 ]] && printf '%s|%s|%s|%s\n' "$repo" "$tag" "$image_id" "$image_primary" >>"$tmp"
    done <"$images"
    mv "$tmp" "$images"
    ;;
  *) exit 2 ;;
esac
"#,
    );
}

pub fn write_fake_container_tools(dir: &Path, validation_log: &Path) {
    super::write_exe(&dir.join("etcdctl"), "#!/usr/bin/env bash\nexit 0\n");
    super::write_exe(
        &dir.join("cargo"),
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
case "${{1:-}}:${{2:-}}:${{3:-}}" in
  xtask:task:validate-success)
    printf 'verify-autofix|%s|%s|%s|%s|%s\n' "$PWD" \
      "${{QCOLD_TASKFLOW_CONTEXT:-}}" "${{QCOLD_TASKFLOW_DEVCONTAINER_ID:-}}" \
      "${{QCOLD_TASKFLOW_CONTAINER_ROOT:-}}" "${{CARGO_TARGET_DIR:-}}" >>"{log}"
    printf 'verify-preflight|%s|%s|%s|%s|%s|%s\n' "$PWD" "$(command -v etcdctl)" \
      "${{QCOLD_TASKFLOW_CONTEXT:-}}" "${{QCOLD_TASKFLOW_DEVCONTAINER_ID:-}}" \
      "${{QCOLD_TASKFLOW_CONTAINER_ROOT:-}}" "${{CARGO_TARGET_DIR:-}}" >>"{log}"
    printf 'verify-fast|%s|%s|%s|%s|%s\n' "$PWD" \
      "${{QCOLD_TASKFLOW_CONTEXT:-}}" "${{QCOLD_TASKFLOW_DEVCONTAINER_ID:-}}" \
      "${{QCOLD_TASKFLOW_CONTAINER_ROOT:-}}" "${{CARGO_TARGET_DIR:-}}" >>"{log}"
    exit 0
    ;;
  xtask:verify:autofix)
    printf 'verify-autofix|%s|%s|%s|%s|%s\n' "$PWD" \
      "${{QCOLD_TASKFLOW_CONTEXT:-}}" "${{QCOLD_TASKFLOW_DEVCONTAINER_ID:-}}" \
      "${{QCOLD_TASKFLOW_CONTAINER_ROOT:-}}" "${{CARGO_TARGET_DIR:-}}" >>"{log}"
    exit 0
    ;;
  xtask:verify:preflight)
    printf 'verify-preflight|%s|%s|%s|%s|%s|%s\n' "$PWD" "$(command -v etcdctl)" \
      "${{QCOLD_TASKFLOW_CONTEXT:-}}" "${{QCOLD_TASKFLOW_DEVCONTAINER_ID:-}}" \
      "${{QCOLD_TASKFLOW_CONTAINER_ROOT:-}}" "${{CARGO_TARGET_DIR:-}}" >>"{log}"
    exit 0
    ;;
  xtask:verify:fast)
    printf 'verify-fast|%s|%s|%s|%s|%s\n' "$PWD" \
      "${{QCOLD_TASKFLOW_CONTEXT:-}}" "${{QCOLD_TASKFLOW_DEVCONTAINER_ID:-}}" \
      "${{QCOLD_TASKFLOW_CONTAINER_ROOT:-}}" "${{CARGO_TARGET_DIR:-}}" >>"{log}"
    exit 0
    ;;
esac
printf 'unexpected fake container cargo invocation: %s\n' "$*" >&2
exit 1
"#,
            log = validation_log.display()
        ),
    );
}

pub fn write_fake_devcontainer(path: &Path) {
    super::write_exe(
        path,
        r#"#!/usr/bin/env bash
set -euo pipefail
state=${FAKE_DOCKER_STATE:?}
images=${FAKE_DOCKER_IMAGES:?}
log=${FAKE_DEVCONTAINER_LOG:?}
container_bin=${FAKE_DEVCONTAINER_CONTAINER_BIN:?}
cmd=${1:?}
shift
case "$cmd" in
  up|exec)
    workspace=
    config=
    primary=
    task=
    dev_id=
    remote_env=()
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --workspace-folder) workspace=$2; shift 2 ;;
        --config) config=$2; shift 2 ;;
        --id-label)
          case "$2" in
            taskflow.primary_repo=*) primary=${2#taskflow.primary_repo=} ;;
            taskflow.task_id=*) task=${2#taskflow.task_id=} ;;
            taskflow.devcontainer_id=*) dev_id=${2#taskflow.devcontainer_id=} ;;
          esac
          shift 2
          ;;
        --remote-env)
          remote_env+=("$2")
          shift 2
          ;;
        --*) shift ;;
        *) break ;;
      esac
    done
    name="devcontainer-$(basename "$workspace")"
    id="cid-$(basename "$workspace")"
    image_repo="vsc-$(basename "$workspace")-1234567890ab"
    image_ref="$image_repo:latest"
    touch "$images" "$state"
    tmp_images="$images.tmp"
    : >"$tmp_images"
    while IFS='|' read -r repo tag image_id image_primary; do
      [[ -n "$repo" ]] || continue
      [[ "$repo" == "$image_repo" ]] && continue
      printf '%s|%s|%s|%s\n' "$repo" "$tag" "$image_id" "$image_primary" >>"$tmp_images"
    done <"$images"
    printf '%s|latest|img-%s|%s\n' "$image_repo" "$(basename "$workspace")" "$primary" >>"$tmp_images"
    mv "$tmp_images" "$images"

    tmp_state="$state.tmp"
    : >"$tmp_state"
    while IFS='|' read -r item_id item_name item_workspace item_primary item_task item_dev_id image item_taskflow item_legacy; do
      [[ -n "$item_id" ]] || continue
      [[ "$item_id" == "$id" ]] && continue
      printf '%s|%s|%s|%s|%s|%s|%s|%s|%s\n' \
        "$item_id" "$item_name" "$item_workspace" "$item_primary" "$item_task" "$item_dev_id" "$image" "$item_taskflow" "$item_legacy" \
        >>"$tmp_state"
    done <"$state"
    printf '%s|%s|%s|%s|%s|%s|%s|1|0\n' \
      "$id" "$name" "$workspace" "$primary" "$task" "$dev_id" "$image_ref" >>"$tmp_state"
    mv "$tmp_state" "$state"
    printf '%s|%s|%s|%s|%s\n' "$cmd" "$workspace" "$config" "$task" "$dev_id" >>"$log"
    if [[ "$cmd" == exec ]]; then
      printf 'cmd|%s|%s\n' "$workspace" "$*" >>"$log"
      cd "$workspace"
      unset QCOLD_TASKFLOW_CONTAINER_ROOT QCOLD_TASKFLOW_CONTEXT \
        QCOLD_TASKFLOW_PRIMARY_REPO_PATH QCOLD_TASKFLOW_TASK_ID \
        QCOLD_TASKFLOW_TASK_WORKTREE QCOLD_TASKFLOW_TASK_BRANCH \
        QCOLD_TASKFLOW_DEVCONTAINER_ID CARGO_TARGET_DIR
      for entry in "${remote_env[@]}"; do
        export "$entry"
      done
      PATH="$container_bin:$PATH" "$@"
    fi
    ;;
  *) exit 2 ;;
esac
"#,
    );
}

pub fn write_fake_glab(path: &Path) {
    super::write_exe(
        path,
        r#"#!/usr/bin/env bash
set -euo pipefail
state=${FAKE_GLAB_STATE:?}
cmd=${1:?}
shift

decode() {
  local value=${1:-}
  printf '%b' "${value//%/\\x}"
}

load_state() {
  IID=1
  STATE=
  SOURCE_BRANCH=
  TARGET_BRANCH=
  WEB_URL=
  MERGE_COMMIT_SHA=
  if [[ -s "$state" ]]; then
    # shellcheck disable=SC1090
    source "$state"
  fi
}

save_state() {
  cat >"$state" <<EOF
IID='$IID'
STATE='$STATE'
SOURCE_BRANCH='$SOURCE_BRANCH'
TARGET_BRANCH='$TARGET_BRANCH'
WEB_URL='$WEB_URL'
MERGE_COMMIT_SHA='$MERGE_COMMIT_SHA'
EOF
}

query_value() {
  local query=$1
  local name=$2
  local item key value
  IFS='&' read -r -a items <<<"$query"
  for item in "${items[@]}"; do
    [[ "$item" == *=* ]] || continue
    key=${item%%=*}
    value=${item#*=}
    if [[ "$key" == "$name" ]]; then
      decode "$value"
      return 0
    fi
  done
  return 1
}

field_value() {
  local name=$1
  shift
  local item
  for item in "$@"; do
    [[ "$item" == "$name="* ]] || continue
    printf '%s' "${item#*=}"
    return 0
  done
  return 1
}

merge_remote_branch() {
  local repo=$1
  local remote
  local temp
  remote=$(git -C "$repo" remote get-url origin)
  temp=$(mktemp -d)
  git clone "$remote" "$temp/repo" >/dev/null 2>&1
  git -C "$temp/repo" config user.name tester
  git -C "$temp/repo" config user.email tester@example.com
  git -C "$temp/repo" fetch origin "$SOURCE_BRANCH" "$TARGET_BRANCH" >/dev/null 2>&1
  git -C "$temp/repo" checkout -B "$TARGET_BRANCH" "origin/$TARGET_BRANCH" >/dev/null 2>&1
  git -C "$temp/repo" merge --no-ff --no-edit "origin/$SOURCE_BRANCH" >/dev/null 2>&1
  git -C "$temp/repo" push origin "$TARGET_BRANCH" >/dev/null 2>&1
  MERGE_COMMIT_SHA=$(git -C "$temp/repo" rev-parse HEAD)
  git -C "$temp/repo" push origin --delete "$SOURCE_BRANCH" >/dev/null 2>&1 || true
  rm -rf "$temp"
}

[[ "$cmd" == api ]] || exit 2
endpoint=${1:?}
shift
method=GET
fields=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --method)
      method=${2:?}
      shift 2
      ;;
    -F)
      fields+=("${2:?}")
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

base=${endpoint%%\?*}
query=
if [[ "$endpoint" == *\?* ]]; then
  query=${endpoint#*\?}
fi

load_state

if [[ "$base" == "projects/:id/merge_requests" && "$method" == "GET" ]]; then
  source_branch=$(query_value "$query" source_branch || true)
  target_branch=$(query_value "$query" target_branch || true)
  if [[ "$STATE" == "opened" && "$SOURCE_BRANCH" == "$source_branch" && "$TARGET_BRANCH" == "$target_branch" ]]; then
    printf '[{"iid":%s,"web_url":"%s"}]\n' "$IID" "$WEB_URL"
  else
    printf '[]\n'
  fi
  exit 0
fi

if [[ "$base" == "projects/:id/merge_requests" && "$method" == "POST" ]]; then
  SOURCE_BRANCH=$(field_value source_branch "${fields[@]}")
  TARGET_BRANCH=$(field_value target_branch "${fields[@]}")
  STATE=opened
  WEB_URL="https://gitlab.example.com/sds/main/repository-rs/-/merge_requests/$IID"
  MERGE_COMMIT_SHA=
  save_state
  printf '{"iid":%s,"web_url":"%s"}\n' "$IID" "$WEB_URL"
  exit 0
fi

if [[ "$base" =~ ^projects/:id/merge_requests/([0-9]+)$ ]]; then
  if [[ "$method" == "PUT" ]]; then
    printf '{"iid":%s,"web_url":"%s"}\n' "$IID" "$WEB_URL"
  else
    if [[ "$STATE" == "merged" ]]; then
      printf '{"state":"merged","detailed_merge_status":"mergeable","head_pipeline":{"status":"success"},"merge_commit_sha":"%s"}\n' "$MERGE_COMMIT_SHA"
    else
      printf '{"state":"opened","detailed_merge_status":"mergeable","head_pipeline":{"status":"success"},"merge_commit_sha":null}\n'
    fi
  fi
  exit 0
fi

if [[ "$base" =~ ^projects/:id/merge_requests/([0-9]+)/merge$ && "$method" == "PUT" ]]; then
  merge_remote_branch "$PWD"
  STATE=merged
  save_state
  printf '{"state":"merged","detailed_merge_status":"mergeable","head_pipeline":{"status":"success"},"merge_commit_sha":"%s"}\n' "$MERGE_COMMIT_SHA"
  exit 0
fi

printf 'unexpected fake glab invocation: %s %s\n' "$cmd" "$endpoint" >&2
exit 2
"#,
    );
}
