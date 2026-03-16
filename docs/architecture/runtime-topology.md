# Runtime topology

CosmoBrowse は Browser Core（Rust）と Renderer UI の 2 層で動作します。
現状は Tauri/WebView 実装ですが、目標は Rust ネイティブ描画（WebView 非依存）です。

> Diagram source: `docs/architecture/mermaid/runtime-topology.mmd`

```mermaid
flowchart LR
  UI["Renderer UI (adapter_tauri / adapter_native)\n- URL入力/タブ操作\n- scene_items 描画\n- diagnostics 表示"]
  IPC["Adapter Boundary\n- dispatch_ipc (互換レイヤ)\n- open_url / activate_link"]
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
- UI は `scene_items` を描画し、クリックイベントを adapter 境界経由で Core に戻します。

## Migration notes (WebView 非依存化)
- `adapter_tauri` は移行期間の互換実装として維持し、標準経路を `adapter_native` へ段階移行します。
- 互換性のため `dispatch_ipc` 契約は維持し、フロントの呼び出し面を先に安定化します。
- 最終的には WebView ランタイム必須要件を外し、Rust ネイティブ描画を既定にします。
