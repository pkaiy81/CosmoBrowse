# Render Pipeline

## Current (SceneItem native pipeline)

```mermaid
flowchart LR
  A["HTML LS parsing + DOM Standard tree"] --> B["CSS Display/CSS2 visual formatting model layout"]
  B --> C["saba_core layout/render tree"]
  C --> D["saba_app DTO: FrameViewModel + scene_items"]
  D --> E["cosmo-browse-ui RenderBackend resolver"]
  E --> F["NativeSceneBackend"]
  F --> G["SceneItem raster/compositor"]
```

## Legacy (deprecated WebView compatibility path)

```mermaid
flowchart LR
  A["saba_core layout/render tree"] --> B["saba_app DTO: FrameViewModel"]
  B --> C["cosmo-browse-ui RenderBackend resolver"]
  C --> D["WebViewBackend (deprecated)"]
  D --> E["iframe srcdoc"]
  E --> F["Platform WebView paints pixels"]
```

## Backend swap points

- `FrameViewModel.render_backend` is the backend selection hint transported from `saba_app`.
- `resolveRenderBackend(frame)` in `main.ts` is the single switch point for backend replacement.
- `RenderBackend.renderLeafFrame(...)` now resolves to scene item rendering only; WebView is deprecated.

## Current implementation status

- `NativeSceneBackend`: renders `scene_items` directly into positioned DOM nodes (`rect`, `text`, `image`) without `iframe srcdoc`.
- `WebViewBackend`: deprecated and no longer used for leaf-frame rendering in the UI pipeline.
- The effective runtime rendering path is scene-only, aligned with engine-produced paint order.
