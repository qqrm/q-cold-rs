#!/usr/bin/env python3
"""Summarize Q-COLD or xtask task-flow ZIP bundles without extracting them."""

from __future__ import annotations

import json
import os
import re
import sys
import zipfile
from dataclasses import dataclass
from pathlib import Path


ENV_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$")


@dataclass
class BundleSummary:
    path: Path
    sequence: str = ""
    task: str = ""
    status: str = ""
    profile: str = ""
    head: str = ""
    delivered: str = ""
    merged: str = ""
    outcome: str = ""
    failure: str = ""
    flow: str = ""


def read_text(zf: zipfile.ZipFile, name: str) -> str:
    try:
        with zf.open(name) as handle:
            return handle.read().decode("utf-8", errors="replace")
    except KeyError:
        return ""


def parse_env(text: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for line in text.splitlines():
        match = ENV_RE.match(line.strip())
        if not match:
            continue
        key, value = match.groups()
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
            value = value[1:-1]
        result[key] = value
    return result


def first_failure_phase(summary: dict) -> str:
    for phase in summary.get("phases") or []:
        if phase.get("exit_code") in (None, 0):
            continue
        name = str(phase.get("phase") or "")
        code = phase.get("exit_code")
        tail = phase.get("failure_tail") or []
        suffix = ""
        if tail:
            suffix = f": {str(tail[-1])[:120]}"
        return f"{name} exit={code}{suffix}"
    return ""


def flow_problem(summary: dict, fallback_text: str) -> str:
    groups = (summary.get("flow_problems") or {}).get("top_groups") or []
    if groups:
        group = groups[0]
        text = str(group.get("summary") or "")
        count = group.get("count")
        return f"{text} ({count}x)" if count else text
    for line in fallback_text.splitlines():
        line = line.strip()
        if line.startswith("- ["):
            return line[:160]
    return ""


def summarize(path: Path) -> BundleSummary:
    item = BundleSummary(path=path)
    with zipfile.ZipFile(path) as zf:
        env = parse_env(read_text(zf, "metadata/task.env"))
        if not env:
            env = parse_env(read_text(zf, "metadata/bundle.env"))
        summary_text = read_text(zf, "metadata/task-run-summary.json")
        summary = json.loads(summary_text) if summary_text else {}
        flow_text = read_text(zf, "logs/flow-problems.md")
        comment = zf.comment.decode("utf-8", errors="replace").strip()
        names = zf.namelist()

    item.sequence = env.get("TASK_SEQUENCE", "")
    item.task = env.get("TASK_NAME", "") or str(summary.get("task_name") or "")
    item.status = env.get("STATUS", "") or str(summary.get("outcome") or "")
    item.profile = env.get("TASK_PROFILE", "")
    item.head = env.get("TASK_HEAD", "")[:12]
    item.delivered = env.get("DELIVERED_HEAD", "")[:12]
    item.merged = env.get("MERGED_HEAD", "")[:12]
    item.outcome = str(summary.get("outcome") or "")
    item.failure = first_failure_phase(summary)
    item.flow = flow_problem(summary, flow_text)
    if not env and not summary:
        top = names[0].rstrip("/") if names else path.stem
        item.task = top.split("/", 1)[0]
        item.status = "source-archive"
        item.head = comment[:12]
    return item


def print_table(items: list[BundleSummary]) -> None:
    headers = ["seq", "task", "status", "profile", "head", "delivered", "merged", "failure/flow"]
    rows = []
    for item in items:
        rows.append(
            [
                item.sequence,
                item.task,
                item.status,
                item.profile,
                item.head,
                item.delivered,
                item.merged,
                item.failure or item.flow,
            ]
        )
    widths = [len(header) for header in headers]
    for row in rows:
        for idx, value in enumerate(row):
            widths[idx] = max(widths[idx], min(len(value), 80))
    print("  ".join(header.ljust(widths[idx]) for idx, header in enumerate(headers)))
    print("  ".join("-" * width for width in widths))
    for row in rows:
        clipped = [value if len(value) <= 80 else value[:77] + "..." for value in row]
        print("  ".join(value.ljust(widths[idx]) for idx, value in enumerate(clipped)))


def main(argv: list[str]) -> int:
    if not argv:
        print("usage: summarize_task_bundles.py <bundle.zip> [...]", file=sys.stderr)
        return 2
    paths = [Path(arg) for arg in argv]
    missing = [str(path) for path in paths if not path.is_file()]
    if missing:
        print("missing bundle(s): " + ", ".join(missing), file=sys.stderr)
        return 1
    items = [summarize(path) for path in sorted(paths, key=lambda p: (p.stat().st_mtime, p.name))]
    print_table(items)
    print()
    for item in items:
        print(f"{os.fspath(item.path)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
