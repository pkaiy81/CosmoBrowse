#!/usr/bin/env python3
"""Run download manager regression checks (pause/resume/retry + checksum)."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

IPC_VERSION = 1


@dataclass(frozen=True)
class DownloadCaseResult:
    name: str
    passed: bool
    details: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts-dir", required=True)
    parser.add_argument("--fixture-size-mib", type=int, default=16)
    parser.add_argument("--fixture-port", type=int, default=8765)
    parser.add_argument("--timeout-sec", type=int, default=120)
    return parser.parse_args()


def write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")


def build_native_ipc_cli(repo_root: Path, smoke_log: Path) -> Path:
    saba_dir = repo_root / "saba"
    result = subprocess.run(
        ["cargo", "build", "-p", "adapter_native", "--bin", "native_ipc_cli"],
        cwd=saba_dir,
        capture_output=True,
        text=True,
        check=False,
    )
    smoke_log.parent.mkdir(parents=True, exist_ok=True)
    with smoke_log.open("a", encoding="utf-8") as log:
        log.write(result.stdout)
        log.write(result.stderr)
    if result.returncode != 0:
        tail = "\n".join((result.stdout + "\n" + result.stderr).splitlines()[-40:])
        raise RuntimeError(f"native_ipc_cli build failed:\n{tail}")
    return saba_dir / "target" / "debug" / "native_ipc_cli"


class IpcSession:
    def __init__(self, saba_dir: Path, ipc_cli: Path, log_path: Path) -> None:
        self._process = subprocess.Popen(
            [str(ipc_cli), "stdin"],
            cwd=saba_dir,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        self._log_path = log_path

    def request(self, req_type: str, payload: dict[str, Any] | None = None) -> dict[str, Any]:
        request: dict[str, Any] = {"version": IPC_VERSION, "type": req_type}
        if payload is not None:
            request["payload"] = payload
        wire = json.dumps(request, ensure_ascii=False)
        self._append_log(f">>> {wire}")

        assert self._process.stdin is not None
        assert self._process.stdout is not None
        self._process.stdin.write(wire + "\n")
        self._process.stdin.flush()

        while True:
            line = self._process.stdout.readline()
            if line == "":
                raise RuntimeError(f"native_ipc_cli exited unexpectedly with {self._process.poll()}")
            line = line.strip()
            if not line:
                continue
            self._append_log(f"<<< {line}")
            try:
                parsed = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "code" in parsed and "message" in parsed and "type" not in parsed:
                raise RuntimeError(f"ipc error {parsed['code']}: {parsed['message']}")
            return parsed

    def close(self) -> None:
        if self._process.poll() is None:
            self._process.terminate()
            try:
                self._process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self._process.kill()

    def _append_log(self, line: str) -> None:
        self._log_path.parent.mkdir(parents=True, exist_ok=True)
        with self._log_path.open("a", encoding="utf-8") as log:
            log.write(line + "\n")


def expected_sha256(file_size: int) -> str:
    digest = hashlib.sha256()
    chunk = bytearray(64 * 1024)
    generated = 0
    while generated < file_size:
        limit = min(len(chunk), file_size - generated)
        for idx in range(limit):
            # Fixture stream rule is `offset % 251`.
            chunk[idx] = (generated + idx) % 251
        digest.update(memoryview(chunk)[:limit])
        generated += limit
    return digest.hexdigest()


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            data = handle.read(1024 * 1024)
            if not data:
                break
            digest.update(data)
    return digest.hexdigest()


def configure_download_policy(session: IpcSession, download_dir: Path) -> None:
    session.request(
        "set_download_default_policy",
        {
            "policy": {
                "directory": str(download_dir),
                "conflict_policy": "overwrite",
                "requires_user_confirmation": False,
            }
        },
    )


def wait_for_terminal_state(
    session: IpcSession, download_id: int, timeout_sec: int
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    deadline = time.time() + timeout_sec
    history: list[dict[str, Any]] = []
    while time.time() < deadline:
        response = session.request("get_download_progress", {"id": download_id})
        payload = response.get("payload", {})
        history.append(payload)
        state = payload.get("state")
        if state in {"completed", "failed", "cancelled", "paused"}:
            return payload, history
        time.sleep(0.1)
    raise TimeoutError(f"download {download_id} did not reach terminal state in {timeout_sec}s")


def run_pause_resume_checksum_case(
    session: IpcSession, *, base_url: str, timeout_sec: int, file_size_bytes: int
) -> DownloadCaseResult:
    enqueue = session.request("enqueue_download", {"url": f"{base_url}/attachment.bin"})
    entry = enqueue.get("payload", {})
    download_id = int(entry["id"])

    # RFC 9110 §9.2.2 allows retrying idempotent methods such as GET.
    # We pause after measurable progress and then resume the same GET transfer.
    # https://www.rfc-editor.org/rfc/rfc9110.html#section-9.2.2
    started = time.time()
    paused = False
    while time.time() - started < timeout_sec:
        progress = session.request("get_download_progress", {"id": download_id}).get("payload", {})
        if int(progress.get("downloaded_bytes", 0)) > 0:
            session.request("pause_download", {"id": download_id})
            paused_state, _ = wait_for_terminal_state(session, download_id, timeout_sec)
            if paused_state.get("state") != "paused":
                return DownloadCaseResult("pause_resume_checksum", False, "pause did not settle to paused")
            paused = True
            break
        time.sleep(0.1)
    if not paused:
        return DownloadCaseResult("pause_resume_checksum", False, "download never progressed before timeout")

    session.request("resume_download", {"id": download_id})
    terminal, _ = wait_for_terminal_state(session, download_id, timeout_sec)
    if terminal.get("state") != "completed":
        return DownloadCaseResult(
            "pause_resume_checksum", False, f"expected completed after resume, got {terminal.get('state')}"
        )

    supports_resume = terminal.get("supports_resume")
    if supports_resume is not True:
        return DownloadCaseResult("pause_resume_checksum", False, f"supports_resume expected true, got {supports_resume}")

    output_path = Path(str(terminal.get("save_path", "")))
    if not output_path.exists():
        return DownloadCaseResult("pause_resume_checksum", False, f"missing downloaded file: {output_path}")
    actual = file_sha256(output_path)
    expected = expected_sha256(file_size_bytes)
    if actual != expected:
        return DownloadCaseResult("pause_resume_checksum", False, "sha256 mismatch after pause/resume")
    return DownloadCaseResult("pause_resume_checksum", True, f"sha256={actual}")


def run_retry_case(session: IpcSession, *, base_url: str, timeout_sec: int) -> DownloadCaseResult:
    first = session.request("enqueue_download", {"url": f"{base_url}/attachment.bin"})
    first_id = int(first.get("payload", {}).get("id"))
    session.request("cancel_download", {"id": first_id})
    cancelled, _ = wait_for_terminal_state(session, first_id, timeout_sec)
    if cancelled.get("state") != "cancelled":
        return DownloadCaseResult("retry_after_cancel", False, "cancel did not settle to cancelled")

    retried = session.request("enqueue_download", {"url": f"{base_url}/attachment.bin"})
    retry_id = int(retried.get("payload", {}).get("id"))
    terminal, _ = wait_for_terminal_state(session, retry_id, timeout_sec)
    if terminal.get("state") != "completed":
        return DownloadCaseResult("retry_after_cancel", False, f"retry did not complete: {terminal.get('state')}")
    return DownloadCaseResult("retry_after_cancel", True, f"retry_download_id={retry_id}")


def main() -> int:
    args = parse_args()
    repo_root = Path(__file__).resolve().parents[1]
    artifacts_dir = repo_root / args.artifacts_dir
    ipc_log = artifacts_dir / "download-ipc-session.log"
    download_dir = artifacts_dir / "downloads"
    file_size = max(1, args.fixture_size_mib) * 1024 * 1024

    try:
        native_ipc_cli = build_native_ipc_cli(repo_root, ipc_log)
    except Exception as error:  # noqa: BLE001
        write_json(
            artifacts_dir / "download_regression_summary.json",
            {"pass": False, "error": f"build_failed: {error}", "cases": []},
        )
        print(f"DOWNLOAD_REGRESSION_FAILURE: build_failed: {error}", file=sys.stderr)
        return 1

    fixture_cmd = [
        sys.executable,
        str(repo_root / "scripts" / "download_fixture_server.py"),
        "--host",
        "127.0.0.1",
        "--port",
        str(args.fixture_port),
        "--size-mib",
        str(args.fixture_size_mib),
        "--chunk-delay-ms",
        "4",
    ]
    fixture = subprocess.Popen(fixture_cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    try:
        # Allow fixture server boot.
        time.sleep(0.5)
        base_url = f"http://127.0.0.1:{args.fixture_port}"
        session = IpcSession(repo_root / "saba", native_ipc_cli, ipc_log)
        try:
            download_dir.mkdir(parents=True, exist_ok=True)
            configure_download_policy(session, download_dir)
            results = [
                run_pause_resume_checksum_case(
                    session, base_url=base_url, timeout_sec=args.timeout_sec, file_size_bytes=file_size
                ),
                run_retry_case(session, base_url=base_url, timeout_sec=args.timeout_sec),
            ]
            summary = {
                "pass": all(item.passed for item in results),
                "fixture": {
                    "base_url": base_url,
                    "size_bytes": file_size,
                    "size_mib": args.fixture_size_mib,
                },
                "cases": [
                    {"name": item.name, "passed": item.passed, "details": item.details}
                    for item in results
                ],
            }
            write_json(artifacts_dir / "download_regression_summary.json", summary)
            if not summary["pass"]:
                print("DOWNLOAD_REGRESSION_FAILURE: one or more cases failed", file=sys.stderr)
                return 1
            print("Download regression checks passed.")
            return 0
        finally:
            session.close()
    finally:
        fixture.terminate()
        try:
            fixture.wait(timeout=3)
        except subprocess.TimeoutExpired:
            fixture.kill()


if __name__ == "__main__":
    raise SystemExit(main())
