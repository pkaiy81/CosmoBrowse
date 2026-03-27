#!/usr/bin/env python3
"""Evaluate WebView-free GA gates and required consecutive pass streaks."""

from __future__ import annotations

import argparse
import json
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kpi-summary", required=True)
    parser.add_argument("--layout-summary", required=True)
    parser.add_argument("--legacy-usage-summary", required=True)
    parser.add_argument("--download-summary")
    parser.add_argument("--crash-report")
    parser.add_argument("--history-dir")
    parser.add_argument("--history-series", default="webview-free-ga-gate")
    parser.add_argument("--history-key")
    parser.add_argument("--report-out", required=True)
    parser.add_argument("--min-success-rate", type=float, default=0.99)
    parser.add_argument("--max-crash-rate", type=float, default=0.005)
    parser.add_argument("--max-display-time-ms", type=int, default=1500)
    parser.add_argument("--required-consecutive-passes", type=int, default=3)
    return parser.parse_args()


def load_json(path: str | Path) -> Any:
    return json.loads(Path(path).read_text(encoding="utf-8"))


def load_optional_json(path: str | None) -> Any:
    if not path:
        return {}
    file_path = Path(path)
    if not file_path.exists():
        return {}
    return load_json(file_path)


def load_history_reports(history_dir: str | None) -> list[dict[str, Any]]:
    if not history_dir:
        return []
    root = Path(history_dir)
    if not root.exists():
        return []
    reports: list[dict[str, Any]] = []
    for path in sorted(root.rglob("*.json")):
        try:
            payload = load_json(path)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict) and "gate_passed" in payload:
            reports.append(payload)
    reports.sort(key=lambda item: str(item.get("evaluated_at", "")))
    return reports


def consecutive_passes(reports: list[dict[str, Any]]) -> int:
    streak = 0
    for report in reversed(reports):
        if not report.get("gate_passed"):
            break
        streak += 1
    return streak


def build_history_key(explicit_key: str | None, evaluated_at: str) -> str:
    if explicit_key:
        return explicit_key
    compact_timestamp = evaluated_at.replace("-", "").replace(":", "").replace("+00:00", "Z")
    commit = os.environ.get("GITHUB_SHA", "local")[:12]
    run_id = os.environ.get("GITHUB_RUN_ID", "manual")
    return f"{compact_timestamp}--{run_id}--{commit}"


def main() -> int:
    args = parse_args()
    kpi_summary = load_json(args.kpi_summary)
    layout_summary = load_json(args.layout_summary)
    legacy_usage_summary = load_json(args.legacy_usage_summary)
    download_summary = load_json(args.download_summary) if args.download_summary else {}
    crash_report = load_optional_json(args.crash_report)

    success_rate = 1.0 - float(kpi_summary.get("failure_rate", 0.0) or 0.0)
    crash_rate = float(kpi_summary.get("crash_rate", 0.0) or 0.0)
    display_time_ms = int(kpi_summary.get("display_time_ms", 0) or 0)
    layout_pass = bool(layout_summary.get("pass", False))
    unused_legacy_commands = list(legacy_usage_summary.get("unused_legacy_commands", []))
    used_legacy_commands = list(legacy_usage_summary.get("used_legacy_commands", []))
    crash_count = int(((kpi_summary.get("failure_classification", {}) or {}).get("crash", {}) or {}).get("count", 0) or 0)
    crash_required_fields = ["transport", "active_url", "last_command", "build_id", "commit_hash"]
    missing_crash_fields = [field for field in crash_required_fields if not str(crash_report.get(field, "")).strip()]
    crash_metadata_passed = crash_count == 0 or (crash_count == 1 and not missing_crash_fields)
    evaluated_at = datetime.now(tz=timezone.utc).isoformat()
    history_key = build_history_key(args.history_key, evaluated_at)

    checks = [
        {
            "name": "success_rate",
            "actual": success_rate,
            "expected": args.min_success_rate,
            "operator": ">=",
            "passed": success_rate >= args.min_success_rate,
        },
        {
            "name": "crash_rate",
            "actual": crash_rate,
            "expected": args.max_crash_rate,
            "operator": "<=",
            "passed": crash_rate <= args.max_crash_rate,
        },
        {
            "name": "display_time_ms",
            "actual": display_time_ms,
            "expected": args.max_display_time_ms,
            "operator": "<=",
            "passed": display_time_ms <= args.max_display_time_ms,
        },
        {
            "name": "layout_regression",
            "actual": layout_pass,
            "expected": True,
            "operator": "==",
            "passed": layout_pass,
        },
        {
            "name": "legacy_command_reduction",
            "actual": unused_legacy_commands,
            "expected": "at least one unused command removed from compatibility surface",
            "operator": "informational",
            "passed": len(unused_legacy_commands) > 0,
        },
        {
            "name": "download_regression",
            "actual": bool(download_summary.get("pass", False)),
            "expected": True,
            "operator": "==",
            "passed": bool(download_summary.get("pass", False)),
        },
        {
            "name": "crash_exception_metadata",
            "actual": {
                "crash_count": crash_count,
                "missing_fields": missing_crash_fields,
            },
            "expected": "crash_count == 0 OR (crash_count == 1 and required metadata fields are present)",
            "operator": "policy",
            "passed": crash_metadata_passed,
        },
    ]

    mandatory_check_names = {
        "success_rate",
        "crash_rate",
        "display_time_ms",
        "layout_regression",
        "download_regression",
        "crash_exception_metadata",
    }
    gate_passed = all(
        item["passed"] for item in checks if str(item.get("name", "")) in mandatory_check_names
    )
    history_reports = load_history_reports(args.history_dir)
    current_report = {
        "evaluated_at": evaluated_at,
        "history_series": args.history_series,
        "history_key": history_key,
        "gate_passed": gate_passed,
        "required_consecutive_passes": args.required_consecutive_passes,
        "checks": checks,
        "kpis": {
            "success_rate": success_rate,
            "crash_rate": crash_rate,
            "display_time_ms": display_time_ms,
            "fcp_equivalent_ms": int(kpi_summary.get("fcp_equivalent_ms", 0) or 0),
            "memory_usage_kib": kpi_summary.get("memory_usage_kib", {}),
        },
        "layout": layout_summary,
        "legacy_usage": {
            "used_legacy_commands": used_legacy_commands,
            "unused_legacy_commands": unused_legacy_commands,
        },
        "download_regression": download_summary,
        "crash_exception_policy": {
            "crash_count": crash_count,
            "required_fields": crash_required_fields,
            "missing_fields": missing_crash_fields,
            "report_present": bool(crash_report),
        },
    }
    streak = consecutive_passes(history_reports + [current_report])
    current_report["consecutive_pass_streak"] = streak
    current_report["release_blocked"] = streak < args.required_consecutive_passes
    current_report["release_block_reason"] = (
        f"GA gate streak {streak}/{args.required_consecutive_passes} is below required threshold"
        if current_report["release_blocked"]
        else ""
    )

    out_path = Path(args.report_out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(current_report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(current_report, ensure_ascii=False, indent=2))

    return 1 if current_report["release_blocked"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
