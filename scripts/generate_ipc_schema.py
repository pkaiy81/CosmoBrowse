#!/usr/bin/env python3
# Spec: JSON Schema draft 2020-12 — $schema, type, enum, const, required, properties keywords.
# https://json-schema.org/draft/2020-12/json-schema-core
import json
from pathlib import Path

IPC_SCHEMA = {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "title": "CosmoBrowse IPC Contract",
    "schema_version": 1,
    "request_envelope": {
        "type": "object",
        "required": ["version", "type"],
        "properties": {
            "version": {"type": "integer", "const": 1},
            "type": {
                "type": "string",
                "enum": [
                    "open_url",
                    "get_page_view",
                    "set_viewport",
                    "reload",
                    "back",
                    "forward",
                    "activate_link",
                    "get_navigation_state",
                    "get_metrics",
                    "get_latest_crash_report",
                    "new_tab",
                    "switch_tab",
                    "close_tab",
                    "list_tabs",
                    "search",
                    "register_tls_exception",
                    "enqueue_download",
                    "list_downloads",
                    "get_download_progress",
                    "pause_download",
                    "resume_download",
                    "cancel_download",
                    "open_download",
                    "reveal_download",
                ],
            },
            "payload": {"type": "object"},
        },
    },
    "response_envelope": {
        "type": "object",
        "required": ["version", "type", "payload"],
        "properties": {
            "version": {"type": "integer", "const": 1},
            "type": {"type": "string"},
            "payload": {"type": "object"},
        },
    },
    "commands": {
        "open_url": {
            "request_payload": {
                "required": ["url"],
                "properties": {"url": {"type": "string"}},
            },
            "response_type": "page",
        },
        "get_page_view": {"request_payload": None, "response_type": "page"},
        "set_viewport": {
            "request_payload": {
                "required": ["width", "height"],
                "properties": {
                    "width": {"type": "integer"},
                    "height": {"type": "integer"},
                },
            },
            "response_type": "page",
        },
        "reload": {"request_payload": None, "response_type": "page"},
        "back": {"request_payload": None, "response_type": "page"},
        "forward": {"request_payload": None, "response_type": "page"},
        "activate_link": {
            "request_payload": {
                "required": ["frame_id", "href"],
                "properties": {
                    "frame_id": {"type": "string"},
                    "href": {"type": "string"},
                    "target": {"type": ["string", "null"]},
                },
            },
            "response_type": "page",
        },
        "get_navigation_state": {
            "request_payload": None,
            "response_type": "navigation_state",
        },
        "get_metrics": {"request_payload": None, "response_type": "metrics"},
        "get_latest_crash_report": {
            "request_payload": None,
            "response_type": "crash_report",
        },
        "new_tab": {"request_payload": None, "response_type": "tab"},
        "switch_tab": {
            "request_payload": {
                "required": ["id"],
                "properties": {"id": {"type": "integer"}},
            },
            "response_type": "page",
        },
        "close_tab": {
            "request_payload": {
                "required": ["id"],
                "properties": {"id": {"type": "integer"}},
            },
            "response_type": "tabs",
        },
        "list_tabs": {"request_payload": None, "response_type": "tabs"},
        "search": {
            "request_payload": {
                "required": ["query"],
                "properties": {"query": {"type": "string"}},
            },
            "response_type": "search_results",
        },
        "register_tls_exception": {
            "request_payload": {
                "required": ["url"],
                "properties": {"url": {"type": "string"}},
            },
            "response_type": "ack",
        },
        "enqueue_download": {
            "request_payload": {
                "required": ["url"],
                "properties": {"url": {"type": "string"}},
            },
            "response_type": "download",
        },
        "list_downloads": {"request_payload": None, "response_type": "downloads"},
        "get_download_progress": {
            "request_payload": {
                "required": ["id"],
                "properties": {"id": {"type": "integer"}},
            },
            "response_type": "download",
        },
        "pause_download": {
            "request_payload": {
                "required": ["id"],
                "properties": {"id": {"type": "integer"}},
            },
            "response_type": "download",
        },
        "resume_download": {
            "request_payload": {
                "required": ["id"],
                "properties": {"id": {"type": "integer"}},
            },
            "response_type": "download",
        },
        "cancel_download": {
            "request_payload": {
                "required": ["id"],
                "properties": {"id": {"type": "integer"}},
            },
            "response_type": "download",
        },
        "open_download": {
            "request_payload": {
                "required": ["id"],
                "properties": {"id": {"type": "integer"}},
            },
            "response_type": "download",
        },
        "reveal_download": {
            "request_payload": {
                "required": ["id"],
                "properties": {"id": {"type": "integer"}},
            },
            "response_type": "download",
        },
    },
}


def main() -> None:
    out = Path("docs/ipc/ipc-schema-v1.generated.json")
    out.write_text(json.dumps(IPC_SCHEMA, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
