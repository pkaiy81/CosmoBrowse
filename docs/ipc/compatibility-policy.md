# IPC Compatibility Policy (`adapter_native` ↔ `src-tauri`)

## Versioning
- IPC DTO (`IpcRequest`, `IpcResponse`) は `version` フィールドで明示的にバージョン固定する。
- 現行メジャーは `version: 1`。
- `dispatch_ipc` はサポート対象外 `version` を受け取った場合にエラーを返す。

## Backward Compatibility Rules
- **許可**: フィールド追加、command 追加、response 種別追加（既存 command の意味を壊さないもの）。
- **禁止（メジャー更新必須）**:
  - 既存フィールド削除
  - 既存フィールドの型変更
  - 既存 command の削除
  - 既存 command の response 種別変更

## Schema Management
- ソース・オブ・トゥルース: `scripts/generate_ipc_schema.py`
- ベースライン: `docs/ipc/ipc-schema-v1.json`
- CI 生成物: `docs/ipc/ipc-schema-v1.generated.json`
- 互換性チェック: `scripts/check_ipc_compat.py`

## Legacy Tauri Command Migration Map
`dispatch_ipc` へ段階移行するための対応表:

| Legacy `invoke()` command | `dispatch_ipc` request (`version: 1`) |
|---|---|
| `open_url(url)` | `{ "type": "open_url", "payload": { "url": "..." } }` |
| `activate_link(frame_id, href, target)` | `{ "type": "activate_link", "payload": { "frame_id": "...", "href": "...", "target": "..." } }` |
| `get_page_view()` | `{ "type": "get_page_view" }` |
| `set_viewport(width, height)` | `{ "type": "set_viewport", "payload": { "width": 1280, "height": 720 } }` |
| `reload()` | `{ "type": "reload" }` |
| `back()` | `{ "type": "back" }` |
| `forward()` | `{ "type": "forward" }` |
| `get_navigation_state()` | `{ "type": "get_navigation_state" }` |
| `get_metrics()` | `{ "type": "get_metrics" }` |
| `get_latest_crash_report()` | `{ "type": "get_latest_crash_report" }` |
| `new_tab()` | `{ "type": "new_tab" }` |
| `switch_tab(id)` | `{ "type": "switch_tab", "payload": { "id": 1 } }` |
| `close_tab(id)` | `{ "type": "close_tab", "payload": { "id": 1 } }` |
| `list_tabs()` | `{ "type": "list_tabs" }` |
| `search(query)` | `{ "type": "search", "payload": { "query": "rust" } }` |

> `dispatch_ipc` 実呼び出し時は envelope に `version: 1` を必ず含める。
