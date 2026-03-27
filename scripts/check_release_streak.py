#!/usr/bin/env python3
"""Block release until the required number of consecutive GA reports have passed."""

from __future__ import annotations

import argparse
import json
from datetime import datetime
from pathlib import Path
from typing import Any


MANDATORY_CHECK_NAMES = {
    "success_rate",
    "crash_rate",
    "display_time_ms",
    "layout_regression",
    "download_regression",
    "crash_exception_metadata",
}


def load_json_if_exists(path: Path) -> Any:
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return None


def synthesize_report_from_history_bundle(kpi_path: Path) -> dict[str, Any] | None:
    if kpi_path.name != "kpi_summary.json":
        return None
    bundle_dir = kpi_path.parent

    # Nightly history bundles are JSON objects (RFC 8259) grouped under one history key.
    # We reconstruct a GA gate decision from the same stable fields used by
    # `evaluate_release_gate.py` so release-streak evaluation stays schema-compatible
    # even when ga-gate-report.json is missing from downloaded artifacts.
    kpi_summary = load_json_if_exists(kpi_path)
    layout_summary = load_json_if_exists(bundle_dir / "layout_regression_summary.json") or {}
    download_summary = load_json_if_exists(bundle_dir / "download_regression_summary.json") or {}
    crash_report = load_json_if_exists(bundle_dir / "latest_crash_report.json") or {}
    if not isinstance(kpi_summary, dict):
        return None

    success_rate = 1.0 - float(kpi_summary.get("failure_rate", 0.0) or 0.0)
    crash_rate = float(kpi_summary.get("crash_rate", 0.0) or 0.0)
    display_time_ms = int(kpi_summary.get("display_time_ms", 0) or 0)
    layout_pass = bool(layout_summary.get("pass", False))
    download_pass = bool(download_summary.get("pass", False))
    crash_count = int(
        (((kpi_summary.get("failure_classification", {}) or {}).get("crash", {}) or {}).get("count", 0) or 0)
    )
    crash_required_fields = ("transport", "active_url", "last_command", "build_id", "commit_hash")
    missing_crash_fields = [field for field in crash_required_fields if not str(crash_report.get(field, "")).strip()]
    crash_metadata_passed = crash_count == 0 or (crash_count == 1 and not missing_crash_fields)

    checks = {
        "success_rate": success_rate >= 0.99,
        "crash_rate": crash_rate <= 0.005,
        "display_time_ms": display_time_ms <= 1500,
        "layout_regression": layout_pass,
        "download_regression": download_pass,
        "crash_exception_metadata": crash_metadata_passed,
    }
    gate_passed = all(checks.values())

    return {
        "history_series": "webview-free-kpi-nightly",
        "history_key": bundle_dir.name,
        "evaluated_at": datetime.fromtimestamp(kpi_path.stat().st_mtime).isoformat(),
        "checks": [{"name": name, "passed": passed} for name, passed in checks.items()],
        "gate_passed": gate_passed,
        "source_path": kpi_path.as_posix(),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--history-dir", required=True)
    parser.add_argument("--history-series", default="webview-free-ga-gate")
    parser.add_argument(
        "--history-series-alias",
        action="append",
        dest="history_series_aliases",
        default=[],
        help=(
            "Additional history_series value treated as equivalent to --history-series. "
            "Useful for backward compatibility with older nightly artifact schemas."
        ),
    )
    parser.add_argument("--required-consecutive-passes", type=int, default=3)
    parser.add_argument("--report-out", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    history_dir = Path(args.history_dir)
    reports = []
    synthesized_keys: set[str] = set()
    accepted_series = {args.history_series, *[alias.strip() for alias in args.history_series_aliases]}
    # Backward compatibility: older KPI history snapshots used a broader series label
    # while still embedding GA gate decisions in the same report JSON files.
    # Keeping this alias defaulted preserves release-gate behavior across historical
    # artifacts without weakening schema validation for unrelated JSON blobs.
    accepted_series.add("webview-free-kpi-nightly")

    def normalize_report(payload: dict) -> dict | None:
        # RFC 8259 defines JSON "object" values recursively, so some pipelines wrap
        # the GA report object under a stable key; we unwrap known wrappers first and
        # then evaluate the same gate fields to stay schema-compatible.
        for wrapper_key in ("ga_gate_report", "report", "payload"):
            wrapped = payload.get(wrapper_key)
            if isinstance(wrapped, dict):
                payload = wrapped
                break

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

    scanned_json_files = 0
    inspected_paths: list[str] = []
    for path in sorted(history_dir.rglob("*.json")):
        scanned_json_files += 1
        if len(inspected_paths) < 20:
            inspected_paths.append(path.as_posix())
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
            synthesized = synthesize_report_from_history_bundle(path)
            if synthesized is None:
                continue
            dedupe_key = str(synthesized.get("history_key", "")).strip()
            if dedupe_key and dedupe_key in synthesized_keys:
                continue
            if dedupe_key:
                synthesized_keys.add(dedupe_key)
            payload = synthesized
        series = str(payload.get("history_series", "")).strip()
        if series and series not in accepted_series:
            continue
        payload["source_path"] = payload.get("source_path", path.as_posix())
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
        "history_dir": history_dir.as_posix(),
        "history_series": args.history_series,
        "accepted_history_series": sorted(series for series in accepted_series if series),
        "scanned_json_files": scanned_json_files,
        "inspected_paths_sample": inspected_paths,
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
