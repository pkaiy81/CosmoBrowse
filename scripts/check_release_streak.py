#!/usr/bin/env python3
"""Block release until the required number of consecutive GA reports have passed."""

from __future__ import annotations

import argparse
import json
from datetime import datetime
from pathlib import Path


MANDATORY_CHECK_NAMES = {
    "success_rate",
    "crash_rate",
    "display_time_ms",
    "layout_regression",
    "download_regression",
    "crash_exception_metadata",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--history-dir", required=True)
    parser.add_argument("--history-series", default="webview-free-ga-gate")
    parser.add_argument("--required-consecutive-passes", type=int, default=3)
    parser.add_argument("--report-out", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    history_dir = Path(args.history_dir)
    reports = []

    def normalize_report(payload: dict) -> dict | None:
        if "gate_passed" in payload:
            return payload

        # Backward compatibility for older GA gate snapshots that predate explicit
        # `gate_passed`. JSON payloads are still RFC 8259 objects, and we infer
        # gate status from stable fields only.
        release_blocked = payload.get("release_blocked")
        if isinstance(release_blocked, bool):
            hydrated = dict(payload)
            hydrated["gate_passed"] = not release_blocked
            return hydrated

        checks = payload.get("checks")
        if isinstance(checks, list):
            check_map: dict[str, bool] = {}
            for item in checks:
                if not isinstance(item, dict):
                    continue
                name = str(item.get("name", "")).strip()
                if not name:
                    continue
                check_map[name] = bool(item.get("passed"))
            if MANDATORY_CHECK_NAMES.issubset(check_map.keys()):
                hydrated = dict(payload)
                hydrated["gate_passed"] = all(check_map[name] for name in MANDATORY_CHECK_NAMES)
                return hydrated

        return None

    for path in sorted(history_dir.rglob("*.json")):
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            continue
        # JSON history artifacts follow RFC 8259. We only treat top-level objects
        # with a GA decision key (`gate_passed`) as streak inputs so that unrelated
        # JSON files (e.g., KPI snapshots, manifests) do not skew release gating.
        if not isinstance(payload, dict):
            continue
        payload = normalize_report(payload)
        if payload is None:
            continue
        series = str(payload.get("history_series", "")).strip()
        if series and series != args.history_series:
            continue
        payload["source_path"] = path.as_posix()
        reports.append(payload)
    def evaluated_at_key(report: dict) -> tuple[int, datetime]:
        raw = str(report.get("evaluated_at", "")).strip()
        if not raw:
            return (1, datetime.min)
        # RFC 3339 allows the UTC designator "Z". Python's fromisoformat expects "+00:00",
        # so we normalize it before parsing to keep chronological ordering spec-compliant.
        normalized = raw.replace("Z", "+00:00")
        try:
            return (0, datetime.fromisoformat(normalized))
        except ValueError:
            return (1, datetime.min)

    reports.sort(key=evaluated_at_key)

    streak = 0
    for report in reversed(reports):
        if not report.get("gate_passed"):
            break
        streak += 1

    result = {
        "reports_found": len(reports),
        "history_series": args.history_series,
        "history_keys": [report.get("history_key", "") for report in reports],
        "required_consecutive_passes": args.required_consecutive_passes,
        "consecutive_pass_streak": streak,
        "release_blocked": streak < args.required_consecutive_passes,
        "report_paths": [report["source_path"] for report in reports],
        "latest_history_key": reports[-1].get("history_key", "") if reports else "",
        "blocking_reason": (
            "no GA gate report JSON (or compatible fields) was found under --history-dir"
            if not reports
            else "consecutive pass streak is below required threshold"
            if streak < args.required_consecutive_passes
            else ""
        ),
    }
    out_path = Path(args.report_out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 1 if result["release_blocked"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
