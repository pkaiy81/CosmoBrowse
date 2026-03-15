# Runtime topology

CosmoBrowse は Browser Core（Rust）と Renderer UI（Tauri/WebView 上の TS）の 2 層で動作します。

> Diagram source: `docs/architecture/mermaid/runtime-topology.mmd`

```mermaid
flowchart LR
  UI["Renderer UI (cosmo-browse-ui)\n- URL入力/タブ操作\n- scene_items 描画\n- diagnostics 表示"]
  IPC["Tauri IPC\n- dispatch_ipc\n- open_url / activate_link"]
  CORE["Browser Core (cosmo_app_legacy)\n- session/history\n- frameset target解決\n- snapshot 生成"]
  NET["Loader / Security\n- reqwest fetch (HTTP/HTTPS)\n- redirect/cache/charset\n- CORS/sandbox/cookie診断"]
  EXT["Remote Origins\n- HTTP/HTTPS servers"]

  UI <--> IPC
  IPC <--> CORE
  CORE <--> NET
  NET <--> EXT
```

## Boundary rules
- UI は DTO（`PageViewModel`, `FrameViewModel`）のみを受け取り、ローダー実装には依存しません。
- HTTPS 通信、文字コード判定、フレームターゲット解決は Rust 側で完結します。
- UI は `scene_items` を描画し、クリックイベントを IPC 経由で Core に戻します。
