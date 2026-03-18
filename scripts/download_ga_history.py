#!/usr/bin/env python3
"""Download recent GA gate report artifacts from GitHub Actions."""

from __future__ import annotations

import argparse
import io
import json
import os
import urllib.request
import zipfile
from pathlib import Path
from typing import Any


API_ROOT = "https://api.github.com"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument("--token", default=os.environ.get("GITHUB_TOKEN"))
    parser.add_argument("--artifact-name", default="ga-gate-nightly")
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


def main() -> int:
    args = parse_args()
    if not args.repo or not args.token:
        raise SystemExit("Both --repo and --token (or matching environment variables) are required")

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    artifacts = api_get(f"{API_ROOT}/repos/{args.repo}/actions/artifacts?per_page=100", args.token)
    matches = [
        artifact
        for artifact in artifacts.get("artifacts", [])
        if artifact.get("name") == args.artifact_name and not artifact.get("expired", True)
    ]
    matches.sort(key=lambda artifact: artifact.get("created_at", ""), reverse=True)

    extracted = 0
    for artifact in matches[: args.limit]:
        artifact_dir = output_dir / f"{artifact['id']}"
        artifact_dir.mkdir(parents=True, exist_ok=True)
        archive = download_bytes(artifact["archive_download_url"], args.token)
        with zipfile.ZipFile(io.BytesIO(archive)) as zf:
            zf.extractall(artifact_dir)
        extracted += 1

    print(json.dumps({"downloaded_artifacts": extracted, "output_dir": output_dir.as_posix()}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
