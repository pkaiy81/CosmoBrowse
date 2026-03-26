#!/usr/bin/env python3
"""Run a local preflight approximation of GitHub Actions CI workflows."""

from __future__ import annotations

import argparse
import os
import shlex
import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SABA_DIR = REPO_ROOT / "saba"
FRONTEND_DIR = REPO_ROOT / "saba" / "ui" / "cosmo-browse-ui"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--skip-rust",
        action="store_true",
        help="Skip Rust checks (cargo check/test)",
    )
    parser.add_argument(
        "--skip-frontend",
        action="store_true",
        help="Skip frontend checks (npm ci/build)",
    )
    parser.add_argument(
        "--skip-smoke",
        action="store_true",
        help="Skip PR smoke regression + gate evaluation",
    )
    parser.add_argument(
        "--with-release-gate",
        action="store_true",
        help=(
            "Also run release-gate streak validation by downloading recent GA history. "
            "Requires GITHUB_TOKEN and GITHUB_REPOSITORY."
        ),
    )
    parser.add_argument(
        "--history-limit",
        type=int,
        default=3,
        help="How many artifacts to fetch for --with-release-gate (default: 3)",
    )
    parser.add_argument(
        "--release-history-dir",
        help=(
            "Existing GA history directory for --with-release-gate. "
            "When provided, skip GitHub artifact download and read local JSON files."
        ),
    )
    return parser.parse_args()


def run(cmd: list[str], cwd: Path | None = None, env: dict[str, str] | None = None) -> None:
    print(f"\n==> {shlex.join(cmd)}")
    subprocess.run(cmd, cwd=cwd, env=env, check=True)


def ensure_pr_download_placeholder(artifacts_dir: Path) -> Path:
    summary_path = artifacts_dir / "download_regression_summary.json"
    summary_path.parent.mkdir(parents=True, exist_ok=True)
    summary_path.write_text(
        """{
  "pass": true,
  "mode": "pr_placeholder",
  "note": "Full download regression runs in nightly workflow.",
  "cases": []
}
""",
        encoding="utf-8",
    )
    return summary_path


def main() -> int:
    args = parse_args()

    if not args.skip_rust:
        run(
            [
                "cargo",
                "check",
                "-p",
                "cosmo_core_legacy",
                "-p",
                "cosmo_app_legacy",
                "-p",
                "adapter_cli",
            ],
            cwd=SABA_DIR,
        )
        run(
            [
                "cargo",
                "test",
                "-p",
                "cosmo_core_legacy",
                "-p",
                "cosmo_app_legacy",
                "-p",
                "adapter_cli",
            ],
            cwd=SABA_DIR,
        )

    if not args.skip_frontend:
        run(["npm", "ci"], cwd=FRONTEND_DIR)
        run(["npm", "run", "build"], cwd=FRONTEND_DIR)

    if not args.skip_smoke:
        pr_artifacts = REPO_ROOT / "smoke-artifacts" / "pr"
        run(
            [
                "python3",
                "scripts/run_smoke_regression.py",
                "--mode",
                "pr",
                "--artifacts-dir",
                pr_artifacts.as_posix(),
            ],
            cwd=REPO_ROOT,
        )
        run(
            [
                "python3",
                "scripts/collect_legacy_command_usage.py",
                "--output",
                (pr_artifacts / "legacy-command-usage.json").as_posix(),
            ],
            cwd=REPO_ROOT,
        )
        download_summary = ensure_pr_download_placeholder(pr_artifacts)
        run(
            [
                "python3",
                "scripts/evaluate_release_gate.py",
                "--kpi-summary",
                (pr_artifacts / "kpi_summary.json").as_posix(),
                "--layout-summary",
                (pr_artifacts / "layout_regression_summary.json").as_posix(),
                "--legacy-usage-summary",
                (pr_artifacts / "legacy-command-usage.json").as_posix(),
                "--download-summary",
                download_summary.as_posix(),
                "--crash-report",
                (pr_artifacts / "latest_crash_report.json").as_posix(),
                "--report-out",
                (pr_artifacts / "ga-gate-report.json").as_posix(),
                "--required-consecutive-passes",
                "1",
            ],
            cwd=REPO_ROOT,
        )

    if args.with_release_gate:
        if args.release_history_dir:
            release_artifacts_dir = Path(args.release_history_dir).resolve()
            if not release_artifacts_dir.exists():
                raise SystemExit(
                    f"--release-history-dir does not exist: {release_artifacts_dir.as_posix()}"
                )
        else:
            token = os.environ.get("GITHUB_TOKEN", "").strip()
            repo = os.environ.get("GITHUB_REPOSITORY", "").strip()
            if not token or not repo:
                raise SystemExit(
                    "--with-release-gate requires GITHUB_TOKEN and GITHUB_REPOSITORY in "
                    "environment, or --release-history-dir"
                )
            release_artifacts_dir = REPO_ROOT / "release-artifacts" / "ga-history"
            run(
                [
                    "python3",
                    "scripts/download_ga_history.py",
                    "--repo",
                    repo,
                    "--token",
                    token,
                    "--output-dir",
                    release_artifacts_dir.as_posix(),
                    "--limit",
                    str(args.history_limit),
                ],
                cwd=REPO_ROOT,
            )
        run(
            [
                "python3",
                "scripts/check_release_streak.py",
                "--history-dir",
                release_artifacts_dir.as_posix(),
                "--report-out",
                (REPO_ROOT / "release-artifacts" / "release-streak-report.json").as_posix(),
                "--required-consecutive-passes",
                "3",
            ],
            cwd=REPO_ROOT,
        )

    print("\nPreflight checks completed successfully.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
