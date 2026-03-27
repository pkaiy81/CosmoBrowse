#!/usr/bin/env python3
"""Large pseudo-file fixture for download pause/resume verification.

This fixture is intentionally separate from `run_smoke_regression.py` so the
normal smoke suite stays fast while download-specific scenarios can exercise
pause/resume against a deterministic large response body.

Endpoints:
* /attachment.bin -> `Content-Disposition: attachment` + `Accept-Ranges: bytes`
* /no-range.bin   -> `Content-Disposition: attachment` without range support

The server emits predictable byte patterns and throttles chunk delivery so the UI
or backend tests have time to issue pause/resume/cancel actions.
"""

from __future__ import annotations

import argparse
import http.server
import socketserver
import time
from typing import Tuple


class DownloadFixtureHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    file_size = 8 * 1024 * 1024
    chunk_size = 32 * 1024
    chunk_delay_ms = 20

    def do_GET(self) -> None:  # noqa: N802
        if self.path not in {"/attachment.bin", "/no-range.bin"}:
            self.send_error(404)
            return

        range_enabled = self.path == "/attachment.bin"
        start, end = self._resolve_range(range_enabled)
        payload_length = end - start + 1
        status = 206 if range_enabled and start > 0 else 200

        self.send_response(status)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Disposition", 'attachment; filename="fixture-download.bin"')
        self.send_header("Content-Length", str(payload_length))
        if range_enabled:
            self.send_header("Accept-Ranges", "bytes")
        if status == 206:
            # RFC 9110 Range / Partial Content alignment: resumed requests include
            # Content-Range so clients can validate the server honored the byte
            # range and safely append to the `.part` file instead of restarting.
            # https://www.rfc-editor.org/rfc/rfc9110.html#name-range
            # https://www.rfc-editor.org/rfc/rfc9110.html#name-partial-content
            self.send_header("Content-Range", f"bytes {start}-{end}/{self.file_size}")
        self.end_headers()

        for offset in range(start, end + 1, self.chunk_size):
            chunk_end = min(offset + self.chunk_size, end + 1)
            payload = bytes((index % 251 for index in range(offset, chunk_end)))
            self.wfile.write(payload)
            self.wfile.flush()
            time.sleep(self.chunk_delay_ms / 1000.0)

    def log_message(self, format: str, *args) -> None:  # noqa: A003
        return

    def _resolve_range(self, range_enabled: bool) -> Tuple[int, int]:
        if not range_enabled:
            return 0, self.file_size - 1
        header = self.headers.get("Range")
        if not header or not header.startswith("bytes="):
            return 0, self.file_size - 1
        start_raw = header.split("=", 1)[1].split("-", 1)[0].strip()
        try:
            start = max(0, min(int(start_raw), self.file_size - 1))
        except ValueError:
            start = 0
        return start, self.file_size - 1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--size-mib", type=int, default=8)
    parser.add_argument("--chunk-delay-ms", type=int, default=20)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    DownloadFixtureHandler.file_size = max(1, args.size_mib) * 1024 * 1024
    DownloadFixtureHandler.chunk_delay_ms = max(0, args.chunk_delay_ms)
    with socketserver.ThreadingTCPServer((args.host, args.port), DownloadFixtureHandler) as server:
        print(f"download fixture serving on http://{args.host}:{args.port}")
        server.serve_forever()


if __name__ == "__main__":
    main()
