#!/usr/bin/env python3
"""Regression smoke for navigation, rendering, and input flows."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import threading
from collections import deque
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
    SmokeCase("static_page", "/static", "Static Fixture", "Welcome static content", 2),
    SmokeCase("lightweight_spa", "/spa", "Lightweight SPA", "Counter: 0", 1),
    SmokeCase("redirect", "/redirect", "Static Fixture", "Welcome static content", 2),
    SmokeCase("error_page", "/error", "Error Fixture", "Something went wrong", 1),
]

PR_CASES = {"static_page", "redirect"}
LAYOUT_BREAKAGE_THRESHOLD = 0.10


class FixtureHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path == "/":
            self._send_html(200, "<html><head><title>Index</title></head><body><main>Fixture root</main></body></html>")
            return
        if path == "/static":
            self._send_html(
                200,
                """
                <html><head><title>Static Fixture</title></head><body><main>
                <h1>Welcome static content</h1><p>Stable DOM for smoke checks.</p>
                <a href='/spa'>Go SPA</a><a href='/error'>Go error</a>
                </main></body></html>
                """,
            )
            return
        if path == "/spa":
            self._send_html(
                200,
                """
                <html><head><title>Lightweight SPA</title></head><body><main>
                <h1>Counter: 0</h1>
                <button id='inc' onclick='document.querySelector("h1").innerText = "Counter: 1"'>+1</button>
                <a href='/static'>Back to static</a>
                </main></body></html>
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
                <html><head><title>Error Fixture</title></head><body><main>
                <h1>Something went wrong</h1><a href='/static'>Try again</a>
                </main></body></html>
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
    def __init__(self, saba_dir: Path, ipc_cli_path: Path, log_path: Path) -> None:
        self._process = subprocess.Popen(
            [str(ipc_cli_path), "stdin"],
            cwd=saba_dir,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        self._log_path = log_path
        self._recent_non_json_lines: deque[str] = deque(maxlen=40)

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
                trailer = "\n".join(self._recent_non_json_lines)
                raise RuntimeError(
                    "IPC process ended before returning JSON "
                    f"(exit_code={self._process.poll()}). Recent output:\n{trailer}"
                )
            response = response.strip()
            if not response:
                continue
            self._append_log(f"<<< {response}")
            try:
                return json.loads(response)
            except json.JSONDecodeError:
                self._recent_non_json_lines.append(response)

    def close(self) -> None:
        if self._process.poll() is not None:
            return
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


def ensure_native_ipc_cli_built(saba_dir: Path, smoke_log: Path) -> Path:
    smoke_log.parent.mkdir(parents=True, exist_ok=True)
    command = ["cargo", "build", "-p", "adapter_native", "--bin", "native_ipc_cli"]
    result = subprocess.run(command, cwd=saba_dir, capture_output=True, text=True)
    if result.stdout:
        with smoke_log.open("a", encoding="utf-8") as log:
            log.write(result.stdout)
    if result.stderr:
        with smoke_log.open("a", encoding="utf-8") as log:
            log.write(result.stderr)
    if result.returncode != 0:
        tail = "\n".join((result.stdout + "\n" + result.stderr).splitlines()[-30:])
        raise RuntimeError(f"Failed to build native_ipc_cli.\n{tail}")

    return saba_dir / "target" / "debug" / "native_ipc_cli"


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
    return len(re.findall(r"<a\b", html, flags=re.IGNORECASE))


def ensure_page(case: SmokeCase, page_response: dict[str, Any], expected_url: str) -> None:
    if page_response.get("type") != "page":
        raise AssertionError(f"{case.name}: expected page response, got {page_response.get('type')}")

    payload = page_response.get("payload", {})
    html = collect_html(payload)
    title = payload.get("title", "")
    current_url = payload.get("current_url", "")

    if title != case.expect_title:
        raise AssertionError(f"{case.name}: expected title '{case.expect_title}', got '{title}'")
    if case.expect_text not in html:
        raise AssertionError(f"{case.name}: expected text '{case.expect_text}' not found")
    if count_links(html) < case.min_links:
        raise AssertionError(f"{case.name}: expected at least {case.min_links} links")

    if case.name == "redirect":
        if not current_url.endswith("/static"):
            raise AssertionError(f"redirect: expected current_url to end with /static, got {current_url}")
    elif current_url != expected_url:
        raise AssertionError(f"{case.name}: expected current_url {expected_url}, got {current_url}")




def collect_layout_issues(page_response: dict[str, Any]) -> list[str]:
    payload = page_response.get("payload", {})
    root_frame = payload.get("root_frame", {})
    tree = root_frame.get("render_tree", {}) or {}
    root = tree.get("root")
    if not isinstance(root, dict):
        return ["missing_render_tree_root"]

    issues: list[str] = []

    def walk(node: dict[str, Any]) -> None:
        box = node.get("box_info") or {}
        children = node.get("children") or []
        width = int(box.get("width", 0))
        height = int(box.get("height", 0))
        content_width = int(box.get("content_width", 0))
        content_height = int(box.get("content_height", 0))

        node_name = str(node.get("node_name", "?"))
        if node_name != "#text" and (width <= 0 or height <= 0):
            issues.append(f"missing_box:{node_name}")

        if content_width > width or content_height > height:
            issues.append(f"overflow_box_model:{node_name}")

        for child in children:
            child_box = child.get("box_info") or {}
            cx = int(child_box.get("x", 0))
            cy = int(child_box.get("y", 0))
            cw = int(child_box.get("width", 0))
            ch = int(child_box.get("height", 0))
            x = int(box.get("x", 0))
            y = int(box.get("y", 0))
            if cw > 0 and ch > 0 and width > 0 and height > 0:
                if cx < x or cy < y or cx + cw > x + width or cy + ch > y + height:
                    issues.append(f"child_out_of_parent:{node_name}->{child.get('node_name', '?')}")
            walk(child)

    walk(root)
    return issues

def run_navigation_and_tab_smoke(session: IpcSession, base_url: str) -> None:
    static_url = f"{base_url}/static"
    spa_url = f"{base_url}/spa"

    if session.request("open_url", {"url": static_url}).get("type") != "page":
        raise AssertionError("navigation scenario: failed to open static page")
    if session.request("open_url", {"url": spa_url}).get("type") != "page":
        raise AssertionError("navigation scenario: failed to open spa page")

    if session.request("back").get("payload", {}).get("title") != "Static Fixture":
        raise AssertionError("navigation scenario: back did not return static page")
    if session.request("forward").get("payload", {}).get("title") != "Lightweight SPA":
        raise AssertionError("navigation scenario: forward did not return spa page")

    original_count = len(session.request("list_tabs").get("payload", []))
    created = session.request("new_tab")
    if not isinstance(created.get("payload", {}).get("id"), int):
        raise AssertionError("tab scenario: new_tab did not return a tab id")

    session.request("open_url", {"url": f"{base_url}/error"})
    if session.request("switch_tab", {"id": 1}).get("payload", {}).get("title") != "Lightweight SPA":
        raise AssertionError("tab scenario: switching back to tab 1 lost previous state")
    if len(session.request("list_tabs").get("payload", [])) < original_count + 1:
        raise AssertionError("tab scenario: expected one more tab after new_tab")


def run_e7_t2_js_runtime_smoke(repo_root: Path) -> None:
    """E7-T2 runtime smoke: static + lightweight SPA boot/event wiring."""
    # Spec: DOMContentLoaded is fired after the document has been parsed.
    # https://html.spec.whatwg.org/multipage/parsing.html#the-end
    # Note: our minimum JS lexer does not yet support `//` comments; keep fixtures comment-free.
    saba_dir = repo_root / "saba"
    cmd_base = ["cargo", "run", "-p", "adapter_cli", "--", "verify-event-loop"]

    static_fixture = saba_dir / "testdata" / "js_event_loop" / "e7_t2_static.html"
    spa_fixture = saba_dir / "testdata" / "js_event_loop" / "e7_t2_spa.html"

    static_run = subprocess.run(
        cmd_base + [str(static_fixture)],
        cwd=saba_dir,
        capture_output=True,
        text=True,
        check=False,
    )
    if static_run.returncode != 0:
        raise AssertionError(f"e7_t2_static runtime smoke failed: {static_run.stderr or static_run.stdout}")
    if "render_pipeline_invalidated: true" not in static_run.stdout:
        raise AssertionError("e7_t2_static expected DOMContentLoaded-driven render invalidation")

    spa_run = subprocess.run(
        cmd_base + [str(spa_fixture), "field"],
        cwd=saba_dir,
        capture_output=True,
        text=True,
        check=False,
    )
    if spa_run.returncode != 0:
        raise AssertionError(f"e7_t2_spa runtime smoke failed: {spa_run.stderr or spa_run.stdout}")
    if "render_pipeline_invalidated: true" not in spa_run.stdout:
        raise AssertionError("e7_t2_spa expected input/change/click dispatch to invalidate render pipeline")


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
    parser.add_argument("--artifacts-dir", default="smoke-artifacts")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo_root = Path(__file__).resolve().parents[1]
    saba_dir = repo_root / "saba"
    artifacts_dir = repo_root / args.artifacts_dir
    smoke_log = artifacts_dir / "ipc-session.log"

    failures: list[str] = []
    snapshots: dict[str, Any] = {}
    layout_failures: dict[str, list[str]] = {}

    try:
        ipc_cli_path = ensure_native_ipc_cli_built(saba_dir, smoke_log)
    except Exception as error:  # noqa: BLE001
        failures.append(f"build_failed: {error}")
        write_json(artifacts_dir / "failures.json", failures)
        print(f"SMOKE_FAILURE: {failures[0]}", file=sys.stderr)
        return 1

    cases = [c for c in REPRESENTATIVE_CASES if args.mode == "nightly" or c.name in PR_CASES]
    server, base_url = start_fixture_server()
    session = IpcSession(saba_dir=saba_dir, ipc_cli_path=ipc_cli_path, log_path=smoke_log)

    try:
        for case in cases:
            url = f"{base_url}{case.path}"
            response = session.request("open_url", {"url": url})
            snapshots[case.name] = response
            try:
                ensure_page(case, response, expected_url=url)
            except AssertionError as error:
                failures.append(str(error))

            issues = collect_layout_issues(response)
            if issues:
                layout_failures[case.name] = issues
        run_navigation_and_tab_smoke(session, base_url)
        run_e7_t2_js_runtime_smoke(repo_root)
    except Exception as error:  # noqa: BLE001
        failures.append(f"runner_exception: {error}")
    finally:
        try:
            write_json(artifacts_dir / "app_metrics_snapshot.json", session.request("get_metrics"))
        except Exception as error:  # noqa: BLE001
            failures.append(f"metrics_collection_failed: {error}")
        total_cases = max(len(cases), 1)
        breakage_rate = len(layout_failures) / total_cases
        summary = {
            "mode": args.mode,
            "cases": [c.name for c in cases],
            "layout_failures": layout_failures,
            "breakage_rate": breakage_rate,
            "threshold": LAYOUT_BREAKAGE_THRESHOLD,
            "pass": breakage_rate <= LAYOUT_BREAKAGE_THRESHOLD,
        }
        write_json(artifacts_dir / "layout_regression_summary.json", summary)
        write_json(artifacts_dir / "page_snapshots.json", snapshots)
        if breakage_rate > LAYOUT_BREAKAGE_THRESHOLD:
            failures.append(
                f"layout_breakage_rate_exceeded: {breakage_rate:.2%} > {LAYOUT_BREAKAGE_THRESHOLD:.2%}"
            )
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
