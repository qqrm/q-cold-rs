#!/bin/sh
set -eu

usage() {
    echo "usage: summarize_task_bundles.sh <bundle.zip> [...]" >&2
}

need_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: missing required command: $1" >&2
        exit 1
    fi
}

read_zip() {
    unzip -p "$1" "$2" 2>/dev/null || true
}

env_value() {
    awk -v key="$1" '
        index($0, key "=") == 1 {
            value = substr($0, length(key) + 2)
            gsub(/^[ \t]+|[ \t]+$/, "", value)
            first = substr(value, 1, 1)
            last = substr(value, length(value), 1)
            if ((first == "\047" && last == "\047") || (first == "\"" && last == "\"")) {
                value = substr(value, 2, length(value) - 2)
            }
            print value
            exit
        }
    '
}

short_value() {
    awk '{ print substr($0, 1, 12); exit }'
}

json_value() {
    query=$1
    if [ "$HAVE_JQ" -ne 1 ] || [ -z "$SUMMARY_JSON" ]; then
        return 0
    fi
    printf '%s\n' "$SUMMARY_JSON" | jq -r "$query" 2>/dev/null | sed '/^null$/d' | sed -n '1p'
}

first_failure_phase() {
    if [ "$HAVE_JQ" -ne 1 ] || [ -z "$SUMMARY_JSON" ]; then
        return 0
    fi
    printf '%s\n' "$SUMMARY_JSON" | jq -r '
        (.phases // [])[]
        | select((.exit_code // 0) != 0)
        | (.phase // "") as $phase
        | (.exit_code | tostring) as $code
        | ((.failure_tail // [])
            | if type == "array" and length > 0 then (.[-1] | tostring) else "" end) as $tail
        | $phase + " exit=" + $code + (if $tail == "" then "" else ": " + ($tail[0:120]) end)
    ' 2>/dev/null | sed -n '1p'
}

flow_problem() {
    if [ "$HAVE_JQ" -eq 1 ] && [ -n "$SUMMARY_JSON" ]; then
        value=$(printf '%s\n' "$SUMMARY_JSON" | jq -r '
            (.flow_problems.top_groups // [])[0] as $group
            | if $group then
                ($group.summary // "")
                + (if $group.count then " (" + ($group.count | tostring) + "x)" else "" end)
              else "" end
        ' 2>/dev/null | sed '/^null$/d' | sed -n '1p')
        if [ -n "$value" ]; then
            printf '%s\n' "$value"
            return 0
        fi
    fi
    printf '%s\n' "$FLOW_TEXT" | awk '
        /^- \[/ {
            gsub(/[ \t]+/, " ")
            print substr($0, 1, 160)
            exit
        }
    '
}

source_archive_name() {
    first=$(zipinfo -1 "$1" 2>/dev/null | sed -n '1p' | sed 's#/$##')
    if [ -n "$first" ]; then
        printf '%s\n' "$first" | sed 's#/.*##'
    else
        basename "$1" .zip
    fi
}

source_archive_head() {
    pattern='s/.*\([0-9a-f]\{12\}\).*/\1/p'
    {
        zipinfo -1 "$1" 2>/dev/null | sed -n '1p'
        basename "$1"
    } | sed -n "$pattern" | sed -n '1p'
}

field_from_env() {
    key=$1
    printf '%s\n' "$TASK_ENV" "$RECEIPT_ENV" | env_value "$key"
}

summarize_bundle() {
    path=$1
    mtime=$(stat -c '%Y' "$path")
    TASK_ENV=$(read_zip "$path" metadata/task.env)
    if [ -z "$TASK_ENV" ]; then
        TASK_ENV=$(read_zip "$path" metadata/bundle.env)
    fi
    RECEIPT_ENV=$(read_zip "$path" metadata/terminal-receipt.env)
    SUMMARY_JSON=$(read_zip "$path" metadata/task-run-summary.json)
    FLOW_TEXT=$(read_zip "$path" logs/flow-problems.md)

    sequence=$(field_from_env TASK_SEQUENCE)
    task=$(field_from_env TASK_NAME)
    if [ -z "$task" ]; then
        task=$(json_value '.task_name // empty')
    fi
    status=$(field_from_env STATUS)
    if [ -z "$status" ]; then
        status=$(json_value '.outcome // empty')
    fi
    if [ -z "$status" ]; then
        status=$(field_from_env OUTCOME)
    fi
    profile=$(field_from_env TASK_PROFILE)
    head=$(field_from_env TASK_HEAD | short_value)
    delivered=$(field_from_env DELIVERED_HEAD | short_value)
    merged=$(field_from_env MERGED_HEAD | short_value)
    failure=$(first_failure_phase)
    flow=$(flow_problem)

    if [ -z "$TASK_ENV" ] && [ -z "$SUMMARY_JSON" ]; then
        task=$(source_archive_name "$path")
        status=source-archive
        head=$(source_archive_head "$path")
    fi
    if [ -n "$failure" ]; then
        detail=$failure
    else
        detail=$flow
    fi

    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$mtime" "$path" "$sequence" "$task" "$status" "$profile" "$head" "$delivered" "$merged" "$detail"
}

print_table() {
    awk -F '\t' '
        function clip(value) {
            return length(value) > 80 ? substr(value, 1, 77) "..." : value
        }
        {
            rows[NR] = $0
            for (i = 3; i <= 10; i++) {
                value = clip($i)
                data[NR, i - 2] = value
                if (length(value) > width[i - 2]) {
                    width[i - 2] = length(value)
                }
            }
        }
        END {
            header[1] = "seq"
            header[2] = "task"
            header[3] = "status"
            header[4] = "profile"
            header[5] = "head"
            header[6] = "delivered"
            header[7] = "merged"
            header[8] = "failure/flow"
            for (i = 1; i <= 8; i++) {
                if (length(header[i]) > width[i]) {
                    width[i] = length(header[i])
                }
            }
            for (i = 1; i <= 8; i++) {
                printf "%-*s%s", width[i], header[i], i == 8 ? ORS : "  "
            }
            for (i = 1; i <= 8; i++) {
                for (j = 1; j <= width[i]; j++) {
                    printf "-"
                }
                printf "%s", i == 8 ? ORS : "  "
            }
            for (row = 1; row <= NR; row++) {
                for (i = 1; i <= 8; i++) {
                    printf "%-*s%s", width[i], data[row, i], i == 8 ? ORS : "  "
                }
            }
        }
    '
}

if [ "$#" -eq 0 ]; then
    usage
    exit 2
fi

need_command awk
need_command basename
need_command jq
need_command sed
need_command sort
need_command stat
need_command unzip
need_command zipinfo

HAVE_JQ=1
missing=0
for path in "$@"; do
    if [ ! -f "$path" ]; then
        echo "missing bundle: $path" >&2
        missing=1
    fi
done
if [ "$missing" -ne 0 ]; then
    exit 1
fi

tmp=${TMPDIR:-/tmp}/qcold-task-bundles-$$.tsv
trap 'rm -f "$tmp"' EXIT HUP INT TERM
: >"$tmp"

for path in "$@"; do
    summarize_bundle "$path" >>"$tmp"
done

sorted=$(sort -t '	' -k1,1n -k2,2 "$tmp")
printf '%s\n' "$sorted" | print_table
printf '\n'
printf '%s\n' "$sorted" | awk -F '\t' '{ print $2 }'
