# Render Pipeline

## Current Pipeline (Native Scene)

> Diagram source: `docs/architecture/mermaid/render-pipeline.mmd`

```mermaid
flowchart LR
  A["fetch_document\n(HTTP/HTTPS + redirect/cache)"] --> B["decode_html_bytes\n(charset detection)"]
  B --> C["DOM/frameset parse\n+ target resolution state"]
  C --> D["build FrameViewModel\n+ scene_items"]
  D --> E["cosmo-browse-ui\nresolveRenderBackend"]
  E --> F["NativeSceneBackend"]
  F --> G["DOM nodes\n(rect/text/image)"]
```

## Stage responsibilities

```mermaid
flowchart TD
  N[Network\n- HTTPS fetch\n- redirect/cache] --> D[Decode\n- header/meta charset]
  D --> P[Parse\n- frameset/frame/noframes\n- link target context]
  P --> S[Snapshot\n- FrameViewModel tree\n- scene_items]
  S --> R[Render\n- absolute positioning\n- click events -> activate_link]
```

## Notes
- 現在の既定経路は Native Scene です。
- `render_backend = web_view` が来ても UI 側は Native Scene にフォールバックします。
