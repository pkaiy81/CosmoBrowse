#!/usr/bin/env python3
"""Regression smoke for navigation, rendering, and input flows.

The harness starts a local HTTP fixture server, drives the native IPC CLI in stdin mode,
and validates representative browsing scenarios.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import threading
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

IPC_VERSION = 1


@dataclass(frozen=True)
class SmokeCase:
    name: str
    path: str
    expect_title: str
    expect_text: str
    min_links: int


REPRESENTATIVE_CASES = [
    SmokeCase(
        name="static_page",
        path="/static",
        expect_title="Static Fixture",
        expect_text="Welcome static content",
        min_links=2,
    ),
    SmokeCase(
        name="lightweight_spa",
        path="/spa",
        expect_title="Lightweight SPA",
        expect_text="Counter: 0",
        min_links=1,
    ),
    SmokeCase(
        name="redirect",
        path="/redirect",
        expect_title="Static Fixture",
        expect_text="Welcome static content",
        min_links=2,
    ),
    SmokeCase(
        name="error_page",
        path="/error",
        expect_title="Error Fixture",
        expect_text="Something went wrong",
        min_links=1,
    ),
]

PR_CASES = {"static_page", "redirect"}


class FixtureHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path == "/":
            self._send_html(
                200,
                """
                <html><head><title>Index</title></head>
                <body><main>Fixture root</main><a href='/static'>Static</a></body></html>
                """,
            )
            return
        if path == "/static":
            self._send_html(
                200,
                """
                <html>
                  <head><title>Static Fixture</title></head>
                  <body>
                    <main>
                      <h1>Welcome static content</h1>
                      <p>Stable DOM for smoke checks.</p>
                      <a href='/spa'>Go SPA</a>
                      <a href='/error'>Go error</a>
                    </main>
                  </body>
                </html>
                """,
            )
            return
        if path == "/spa":
            self._send_html(
                200,
                """
                <html>
                  <head><title>Lightweight SPA</title></head>
                  <body>
                    <main>
                      <h1>Counter: 0</h1>
                      <button id='inc' onclick='document.querySelector("h1").innerText = "Counter: 1"'>+1</button>
                      <a href='/static'>Back to static</a>
                    </main>
                  </body>
                </html>
                """,
            )
            return
        if path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "/static")
            self.end_headers()
            return
        if path == "/error":
            self._send_html(
                500,
                """
                <html>
                  <head><title>Error Fixture</title></head>
                  <body>
                    <main>
                      <h1>Something went wrong</h1>
                      <a href='/static'>Try again</a>
                    </main>
                  </body>
                </html>
                """,
            )
            return
        self._send_html(404, "<html><head><title>Not Found</title></head><body>404</body></html>")

    def log_message(self, fmt: str, *args: Any) -> None:
        _ = (fmt, args)

    def _send_html(self, status: int, html: str) -> None:
        body = html.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class IpcSession:
    def __init__(self, saba_dir: Path, log_path: Path) -> None:
        self._process = subprocess.Popen(
            ["cargo", "run", "-p", "adapter_native", "--bin", "native_ipc_cli", "--", "stdin"],
            cwd=saba_dir,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        self._log_path = log_path

    def request(self, request_type: str, payload: dict[str, Any] | None = None) -> dict[str, Any]:
        request: dict[str, Any] = {"version": IPC_VERSION, "type": request_type}
        if payload is not None:
            request["payload"] = payload

        wire = json.dumps(request, ensure_ascii=False)
        self._append_log(f">>> {wire}")

        if self._process.stdin is None or self._process.stdout is None:
            raise RuntimeError("IPC process streams are unavailable")

        self._process.stdin.write(wire + "\n")
        self._process.stdin.flush()

        while True:
            response = self._process.stdout.readline()
            if response == "":
                raise RuntimeError("IPC process ended before a JSON response was returned")
            response = response.strip()
            if not response:
                continue
            self._append_log(f"<<< {response}")
            try:
                return json.loads(response)
            except json.JSONDecodeError:
                # cargo/rustup informational lines can be interleaved on stdout.
                continue

    def close(self) -> None:
        self._process.terminate()
        try:
            self._process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self._process.kill()
            self._process.wait(timeout=3)

    def _append_log(self, line: str) -> None:
        self._log_path.parent.mkdir(parents=True, exist_ok=True)
        with self._log_path.open("a", encoding="utf-8") as f:
            f.write(line + "\n")


def collect_html(page_payload: dict[str, Any]) -> str:
    chunks = [page_payload.get("title", "")]
    for entry in page_payload.get("dom_snapshot", []):
        chunks.append(entry.get("html", ""))
    root = page_payload.get("root_frame", {})
    html_content = root.get("html_content")
    if isinstance(html_content, str):
        chunks.append(html_content)
    return "\n".join(chunks)


def count_links(html: str) -> int:
    return len(re.findall(r"<a\\b", html, flags=re.IGNORECASE))


def ensure_page(case: SmokeCase, page_response: dict[str, Any], expected_url: str) -> None:
    if page_response.get("type") != "page":
        raise AssertionError(f"{case.name}: expected page response, got {page_response.get('type')}")

    payload = page_response.get("payload", {})
    html = collect_html(payload)
    title = payload.get("title", "")
    current_url = payload.get("current_url", "")
    link_count = count_links(html)

    if title != case.expect_title:
        raise AssertionError(f"{case.name}: expected title '{case.expect_title}', got '{title}'")
    if case.expect_text not in html:
        raise AssertionError(f"{case.name}: expected text '{case.expect_text}' not found")
    if link_count < case.min_links:
        raise AssertionError(
            f"{case.name}: expected at least {case.min_links} links, got {link_count}"
        )
    if case.name == "redirect":
        if not current_url.endswith("/static"):
            raise AssertionError(f"redirect: expected current_url to end with /static, got {current_url}")
    elif current_url != expected_url:
        raise AssertionError(f"{case.name}: expected current_url {expected_url}, got {current_url}")


def run_navigation_and_tab_smoke(session: IpcSession, base_url: str) -> None:
    static_url = f"{base_url}/static"
    spa_url = f"{base_url}/spa"

    first = session.request("open_url", {"url": static_url})
    if first.get("type") != "page":
        raise AssertionError("navigation scenario: failed to open static page")

    second = session.request("open_url", {"url": spa_url})
    if second.get("type") != "page":
        raise AssertionError("navigation scenario: failed to open spa page")

    back = session.request("back")
    if back.get("payload", {}).get("title") != "Static Fixture":
        raise AssertionError("navigation scenario: back did not return static page")

    forward = session.request("forward")
    if forward.get("payload", {}).get("title") != "Lightweight SPA":
        raise AssertionError("navigation scenario: forward did not return spa page")

    original_tabs = session.request("list_tabs")
    original_count = len(original_tabs.get("payload", []))

    created = session.request("new_tab")
    new_tab_id = created.get("payload", {}).get("id")
    if not isinstance(new_tab_id, int):
        raise AssertionError("tab scenario: new_tab did not return a tab id")

    session.request("open_url", {"url": f"{base_url}/error"})
    switched_back = session.request("switch_tab", {"id": 1})
    if switched_back.get("payload", {}).get("title") != "Lightweight SPA":
        raise AssertionError("tab scenario: switching back to tab 1 lost previous state")

    final_tabs = session.request("list_tabs")
    if len(final_tabs.get("payload", [])) < original_count + 1:
        raise AssertionError("tab scenario: expected one more tab after new_tab")


def start_fixture_server() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), FixtureHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address
    return server, f"http://{host}:{port}"


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run CosmoBrowse regression smoke checks")
    parser.add_argument("--mode", choices=["pr", "nightly"], default="pr")
    parser.add_argument(
        "--artifacts-dir",
        default="smoke-artifacts",
        help="Directory for logs, snapshots, and metrics",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo_root = Path(__file__).resolve().parents[1]
    saba_dir = repo_root / "saba"
    artifacts_dir = repo_root / args.artifacts_dir
    smoke_log = artifacts_dir / "ipc-session.log"

    cases = [c for c in REPRESENTATIVE_CASES if args.mode == "nightly" or c.name in PR_CASES]

    server, base_url = start_fixture_server()
    session = IpcSession(saba_dir=saba_dir, log_path=smoke_log)

    failures: list[str] = []
    snapshots: dict[str, Any] = {}

    try:
        for case in cases:
            url = f"{base_url}{case.path}"
            response = session.request("open_url", {"url": url})
            snapshots[case.name] = response
            try:
                ensure_page(case, response, expected_url=url)
            except AssertionError as error:
                failures.append(str(error))

        try:
            run_navigation_and_tab_smoke(session, base_url)
        except AssertionError as error:
            failures.append(str(error))
    except Exception as error:  # noqa: BLE001
        failures.append(f"runner_exception: {error}")
    finally:
        try:
            metrics = session.request("get_metrics")
            write_json(artifacts_dir / "app_metrics_snapshot.json", metrics)
        except Exception as error:  # noqa: BLE001
            failures.append(f"metrics_collection_failed: {error}")
        write_json(artifacts_dir / "page_snapshots.json", snapshots)
        if failures:
            write_json(artifacts_dir / "failures.json", failures)
            for line in failures:
                print(f"SMOKE_FAILURE: {line}", file=sys.stderr)

        session.close()
        server.shutdown()

    if failures:
        return 1

    print(f"Smoke checks passed ({args.mode}) with {len(cases)} representative cases.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
