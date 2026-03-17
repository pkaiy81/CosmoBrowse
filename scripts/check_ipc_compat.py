#!/usr/bin/env python3
import json
import sys
from pathlib import Path


def load(path: str):
    return json.loads(Path(path).read_text())


def fail(msg: str):
    print(f"IPC compatibility check failed: {msg}")
    sys.exit(1)


def main() -> None:
    if len(sys.argv) != 3:
        print("usage: check_ipc_compat.py <baseline> <candidate>")
        sys.exit(2)

    baseline = load(sys.argv[1])
    candidate = load(sys.argv[2])

    if baseline["schema_version"] != candidate["schema_version"]:
        fail("schema_version changed. Use a new major versioned schema file.")

    base_cmds = baseline["commands"]
    cand_cmds = candidate["commands"]

    for name, base in base_cmds.items():
        if name not in cand_cmds:
            fail(f"command removed: {name}")
        cand = cand_cmds[name]
        if base["response_type"] != cand["response_type"]:
            fail(f"response_type changed for command: {name}")

        base_payload = base["request_payload"]
        cand_payload = cand["request_payload"]

        if base_payload is None:
            continue
        if cand_payload is None:
            fail(f"payload removed for command: {name}")

        base_required = set(base_payload.get("required", []))
        cand_required = set(cand_payload.get("required", []))
        if not base_required.issubset(cand_required):
            fail(f"required field removed for command: {name}")

        base_props = base_payload.get("properties", {})
        cand_props = cand_payload.get("properties", {})
        for field, base_schema in base_props.items():
            if field not in cand_props:
                fail(f"field removed from {name}: {field}")
            if cand_props[field] != base_schema:
                fail(f"field schema changed in {name}: {field}")

    print("IPC compatibility check passed")


if __name__ == "__main__":
    main()
