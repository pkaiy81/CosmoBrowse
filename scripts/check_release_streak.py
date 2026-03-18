#!/usr/bin/env python3
"""Block release until the required number of consecutive GA reports have passed."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--history-dir", required=True)
    parser.add_argument("--required-consecutive-passes", type=int, default=3)
    parser.add_argument("--report-out", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    history_dir = Path(args.history_dir)
    reports = []
    for path in sorted(history_dir.rglob("ga-gate-report.json")):
        payload = json.loads(path.read_text(encoding="utf-8"))
        payload["source_path"] = path.as_posix()
        reports.append(payload)
    reports.sort(key=lambda item: str(item.get("evaluated_at", "")))

    streak = 0
    for report in reversed(reports):
        if not report.get("gate_passed"):
            break
        streak += 1

    result = {
        "reports_found": len(reports),
        "required_consecutive_passes": args.required_consecutive_passes,
        "consecutive_pass_streak": streak,
        "release_blocked": streak < args.required_consecutive_passes,
        "report_paths": [report["source_path"] for report in reports],
    }
    out_path = Path(args.report_out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 1 if result["release_blocked"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
