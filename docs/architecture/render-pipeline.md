# Render Pipeline

## Current Pipeline (Native Scene + Recovery Path)

> Diagram source: `docs/architecture/mermaid/render-pipeline.mmd`

```mermaid
flowchart LR
  P0["renderer healthcheck\n(ProcessHost)"] -->|healthy| A["fetch_document\n(HTTP/HTTPS + redirect/cache)"]
  P0 -->|unhealthy| P1["restart renderer process\n+ structured recovery log"]
  P1 --> P2["return retryable AppError\n(renderer_recovering)"]
  A --> B["decode_html_bytes\n(charset detection)"]
  B --> C["DOM/frameset parse\n+ target resolution state"]
  C --> D["build FrameViewModel\n+ scene_items"]
  D --> E["cosmo-browse-ui\nresolveRenderBackend"]
  E --> F["NativeSceneBackend"]
  F --> G["DOM nodes\n(rect/text/image)"]
```

## Stage responsibilities

```mermaid
flowchart TD
  H[Health\n- renderer process check\n- restart + recovery log] --> N[Network\n- HTTPS fetch\n- redirect/cache]
  H --> E[Error path\n- renderer_recovering\n- retryable=true]
  N --> D[Decode\n- header/meta charset]
  D --> P[Parse\n- frameset/frame/noframes\n- link target context]
  P --> S[Snapshot\n- FrameViewModel tree\n- scene_items]
  S --> R[Render\n- absolute positioning\n- click events -> activate_link]
```

## Notes
- `adapter_native` は `dispatch` 前に healthcheck を行い、Renderer 異常終了時は自動再起動を試みます。
- 自動再起動後の同一リクエストは失敗として返し、`retryable=true` の `AppError` を UI に返却します。
- 復旧イベントは JSON 構造化ログ（`renderer_recovered` / `renderer_recovery_failed`）で記録します。
- `render_backend = web_view` が来ても UI 側は Native Scene にフォールバックします。
