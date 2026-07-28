#!/usr/bin/env python3
"""Layout reftests: headless-screenshot every page under saba/testdata/reftests
and compare against golden PNGs.

Usage:
  python3 scripts/run_layout_reftests.py            # compare against goldens
  python3 scripts/run_layout_reftests.py --update   # (re)generate goldens
  python3 scripts/run_layout_reftests.py --filter flex   # subset by name

Golden PNGs live in saba/testdata/reftests/golden/<name>.png.
A test fails when more than --max-diff-ratio of pixels differ by more than
--tolerance per channel (see docs/layout-regression-policy.md).

Requires: a debug build of cosmo_browse_native (cargo build -p renderer_native)
and Pillow (python3 -m pip install pillow).
"""

from __future__ import annotations

import argparse
import http.server
import os
import socketserver
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SABA_DIR = REPO_ROOT / "saba"
REFTEST_DIR = SABA_DIR / "testdata" / "reftests"
GOLDEN_DIR = REFTEST_DIR / "golden"
BINARY = SABA_DIR / "target" / "debug" / "cosmo_browse_native"

VIEWPORT = (1024, 768)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--update", action="store_true", help="regenerate golden PNGs")
    p.add_argument("--filter", default="", help="only run tests whose name contains this")
    p.add_argument("--tolerance", type=int, default=0,
                   help="per-channel difference treated as equal (default 0)")
    p.add_argument("--max-diff-ratio", type=float, default=0.0,
                   help="fraction of differing pixels allowed (default 0.0)")
    p.add_argument("--port", type=int, default=8931)
    return p.parse_args()


class _QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def serve(directory: Path, port: int) -> socketserver.TCPServer:
    handler = lambda *a, **kw: _QuietHandler(*a, directory=str(directory), **kw)
    socketserver.TCPServer.allow_reuse_address = True
    httpd = socketserver.TCPServer(("127.0.0.1", port), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def screenshot(url: str, out: Path, snapshot_dir: Path) -> None:
    env = dict(os.environ)
    env["COSMO_SESSION_SNAPSHOT_PATH"] = str(snapshot_dir / "session.json")
    (snapshot_dir / "session.json").unlink(missing_ok=True)
    subprocess.run(
        [str(BINARY), "--screenshot-wh", url, str(out), str(VIEWPORT[0]), str(VIEWPORT[1])],
        env=env,
        check=True,
        capture_output=True,
        timeout=90,
    )


def compare(golden: Path, actual: Path, tolerance: int, max_ratio: float):
    from PIL import Image, ImageChops

    a = Image.open(golden).convert("RGB")
    b = Image.open(actual).convert("RGB")
    if a.size != b.size:
        return False, f"size mismatch {a.size} vs {b.size}"
    diff = ImageChops.difference(a, b)
    if diff.getbbox() is None:
        return True, "identical"
    # Count pixels where any channel differs by more than tolerance.
    hist_bad = 0
    for px in diff.getdata():
        if max(px) > tolerance:
            hist_bad += 1
    ratio = hist_bad / (a.size[0] * a.size[1])
    ok = ratio <= max_ratio
    return ok, f"{hist_bad} px differ ({ratio:.4%}), bbox={diff.getbbox()}"


def main() -> int:
    args = parse_args()
    if not BINARY.exists():
        print(f"error: {BINARY} not found — run `cargo build -p renderer_native` first")
        return 2

    tests = sorted(REFTEST_DIR.glob("*.html"))
    tests = [t for t in tests if args.filter in t.stem]
    if not tests:
        print("no reftests matched")
        return 2

    GOLDEN_DIR.mkdir(parents=True, exist_ok=True)
    httpd = serve(REFTEST_DIR, args.port)
    failures = []
    try:
        with tempfile.TemporaryDirectory() as tmp:
            tmpdir = Path(tmp)
            for test in tests:
                url = f"http://127.0.0.1:{args.port}/{test.name}"
                golden = GOLDEN_DIR / f"{test.stem}.png"
                if args.update:
                    screenshot(url, golden, tmpdir)
                    print(f"UPDATED  {test.stem}")
                    continue
                if not golden.exists():
                    failures.append((test.stem, "no golden (run with --update)"))
                    print(f"MISSING  {test.stem}")
                    continue
                actual = tmpdir / f"{test.stem}.png"
                screenshot(url, actual, tmpdir)
                ok, detail = compare(golden, actual, args.tolerance, args.max_diff_ratio)
                if ok:
                    print(f"PASS     {test.stem}")
                else:
                    fail_out = GOLDEN_DIR.parent / "failures"
                    fail_out.mkdir(exist_ok=True)
                    actual.replace(fail_out / f"{test.stem}.actual.png")
                    failures.append((test.stem, detail))
                    print(f"FAIL     {test.stem}: {detail}")
    finally:
        httpd.shutdown()

    if failures:
        print(f"\n{len(failures)}/{len(tests)} reftests failed "
              f"(actual PNGs in {GOLDEN_DIR.parent / 'failures'})")
        return 1
    print(f"\nall {len(tests)} reftests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
