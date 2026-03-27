#!/usr/bin/env python3
"""Download recent GA gate report artifacts from GitHub Actions."""

from __future__ import annotations

import argparse
import io
import json
import os
import shutil
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path
from typing import Any


API_ROOT = "https://api.github.com"
MAX_API_PAGE_SIZE = 100


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument("--token", default=os.environ.get("GITHUB_TOKEN"))
    parser.add_argument(
        "--artifact-name",
        action="append",
        dest="artifact_names",
        help=(
            "Artifact name to download. Can be specified multiple times. "
            "Defaults to trying smoke-kpi-history-nightly, then ga-gate-nightly."
        ),
    )
    parser.add_argument("--limit", type=int, default=3)
    parser.add_argument("--output-dir", required=True)
    return parser.parse_args()


def api_get(url: str, token: str) -> Any:
    request = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "User-Agent": "CosmoBrowse-GA-History-Downloader",
        },
    )
    with urllib.request.urlopen(request) as response:
        return json.loads(response.read().decode("utf-8"))


def download_bytes(url: str, token: str) -> bytes:
    request = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "User-Agent": "CosmoBrowse-GA-History-Downloader",
        },
    )
    with urllib.request.urlopen(request) as response:
        return response.read()


def load_json_if_exists(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def is_ga_report_payload(payload: Any) -> bool:
    if not isinstance(payload, dict):
        return False
    for wrapper_key in ("ga_gate_report", "report", "payload"):
        wrapped = payload.get(wrapper_key)
        if isinstance(wrapped, dict):
            payload = wrapped
            break
    if "gate_passed" in payload:
        return True
    if isinstance(payload.get("release_blocked"), bool):
        return True
    checks = payload.get("checks")
    return isinstance(checks, list)


def list_artifacts_by_name(repo: str, token: str, artifact_name: str, limit: int) -> list[dict[str, Any]]:
    found: list[dict[str, Any]] = []
    page = 1
    while len(found) < limit:
        # GitHub Actions Artifacts API supports filtering by `name`.
        # Using the API-side filter keeps behavior compliant with GitHub REST API pagination
        # semantics and avoids missing matching artifacts when the global artifact list exceeds 100.
        # Ref: GitHub REST API docs (actions/artifacts list endpoint).
        query = urllib.parse.urlencode(
            {"name": artifact_name, "per_page": MAX_API_PAGE_SIZE, "page": page}
        )
        payload = api_get(f"{API_ROOT}/repos/{repo}/actions/artifacts?{query}", token)
        artifacts = payload.get("artifacts", [])
        if not artifacts:
            break
        for artifact in artifacts:
            # GitHub REST API (Actions Artifacts: list repository artifacts) defines
            # `name` as an optional query filter. Some proxies/GHES versions can return
            # broader result sets, so we defensively enforce exact-name matching here.
            # This keeps release-history selection aligned with the API contract even
            # when upstream filtering is not strictly applied.
            if str(artifact.get("name", "")) != artifact_name:
                continue
            if not artifact.get("expired", True):
                found.append(artifact)
                if len(found) >= limit:
                    break
        if len(artifacts) < MAX_API_PAGE_SIZE:
            break
        page += 1
    return found


def main() -> int:
    args = parse_args()
    if not args.repo or not args.token:
        raise SystemExit("Both --repo and --token (or matching environment variables) are required")

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    artifact_names = args.artifact_names or ["smoke-kpi-history-nightly", "ga-gate-nightly"]

    matches: list[dict[str, Any]] = []
    for artifact_name in artifact_names:
        matches.extend(
            list_artifacts_by_name(
                repo=args.repo,
                token=args.token,
                artifact_name=artifact_name,
                limit=args.limit,
            )
        )

    # GitHub REST API can return overlapping entries across per-name lookups.
    # Keep only the newest instance for each artifact id before extraction.
    deduped: dict[int, dict[str, Any]] = {}
    for artifact in matches:
        artifact_id = int(artifact.get("id", 0) or 0)
        if artifact_id <= 0:
            continue
        current = deduped.get(artifact_id)
        if current is None or str(artifact.get("created_at", "")) > str(current.get("created_at", "")):
            deduped[artifact_id] = artifact
    matches = list(deduped.values())
    matches.sort(key=lambda artifact: artifact.get("created_at", ""), reverse=True)

    extracted = 0
    flattened_reports = 0
    # `--limit` is applied per artifact name during lookup (`list_artifacts_by_name`),
    # so we intentionally extract every deduplicated match here. This keeps the release
    # history complete across both artifact streams (e.g. smoke-kpi-history-nightly and
    # ga-gate-nightly) in line with GitHub Actions artifact list/filter semantics.
    for artifact in matches:
        artifact_dir = output_dir / f"{artifact['id']}"
        artifact_dir.mkdir(parents=True, exist_ok=True)
        archive = download_bytes(artifact["archive_download_url"], args.token)
        with zipfile.ZipFile(io.BytesIO(archive)) as zf:
            zf.extractall(artifact_dir)
        # GitHub Actions upload-artifact wraps paths with directories derived from
        # the uploaded path. We keep the full extraction tree for traceability and
        # also flatten GA report JSON files into --output-dir so downstream
        # check_release_streak.py can consume a stable location regardless of nesting.
        # Ref: GitHub Actions "Store and share data with workflow artifacts" behavior.
        for json_path in artifact_dir.rglob("*.json"):
            payload = load_json_if_exists(json_path)
            if not is_ga_report_payload(payload):
                continue
            flat_name = (
                f"ga-gate-report-{artifact['id']}-"
                f"{flattened_reports + 1:03d}.json"
            )
            flat_path = output_dir / flat_name
            shutil.copyfile(json_path, flat_path)
            flattened_reports += 1
        extracted += 1

    print(
        json.dumps(
            {
                "downloaded_artifacts": extracted,
                "flattened_ga_reports": flattened_reports,
                "output_dir": output_dir.as_posix(),
                "artifact_names": artifact_names,
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
