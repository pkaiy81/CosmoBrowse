#!/usr/bin/env python3
"""Collect direct adapter_tauri legacy-command references from the frontend source tree."""

from __future__ import annotations

import argparse
import json
import re
from collections import Counter
from pathlib import Path

LEGACY_COMMANDS = [
    "open_url",
    "activate_link",
    "get_page_view",
    "set_viewport",
    "reload",
    "back",
    "forward",
    "get_navigation_state",
    "get_metrics",
    "get_latest_crash_report",
    "new_tab",
    "switch_tab",
    "close_tab",
    "list_tabs",
    "search",
]
INVOKE_PATTERN = re.compile(r'(?:legacyInvoke|invoke)\s*<[^>]+>\s*\(\s*"([a-z_]+)"')


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", default="smoke-artifacts/legacy-command-usage.json")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo_root = Path(args.repo_root).resolve()
    frontend_root = repo_root / "saba" / "ui" / "cosmo-browse-ui"
    counts: Counter[str] = Counter()
    files_scanned: list[str] = []

    for path in sorted(frontend_root.rglob("*.ts")):
        if "node_modules" in path.parts:
            continue
        content = path.read_text(encoding="utf-8")
        files_scanned.append(path.relative_to(repo_root).as_posix())
        counts.update(command for command in INVOKE_PATTERN.findall(content) if command in LEGACY_COMMANDS)

    summary = {
        "files_scanned": files_scanned,
        "legacy_command_reference_counts": {command: counts.get(command, 0) for command in LEGACY_COMMANDS},
        "unused_legacy_commands": [command for command in LEGACY_COMMANDS if counts.get(command, 0) == 0],
        "used_legacy_commands": [command for command in LEGACY_COMMANDS if counts.get(command, 0) > 0],
        "total_legacy_references": sum(counts.values()),
    }

    output = repo_root / args.output
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
