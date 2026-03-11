# Render Pipeline

## Current (`WebViewBackend`)

```mermaid
flowchart LR
  A["saba_core layout/render tree"] --> B["saba_app DTO: FrameViewModel"]
  B --> C["cosmo-browse-ui RenderBackend resolver"]
  C --> D["WebViewBackend"]
  D --> E["iframe srcdoc"]
  E --> F["Platform WebView paints pixels"]
```

## Target (`RenderBackend` abstraction)

```mermaid
flowchart LR
  A["saba_core layout/render tree"] --> B["saba_app DTO: FrameViewModel + render_backend/document_url"]
  B --> C["cosmo-browse-ui RenderBackend resolver"]
  C --> D["WebViewBackend (compat)"]
  C --> E["NativeSceneBackend"]
  D --> F["iframe srcdoc"]
  E --> G["SceneItem raster/compositor"]
```

## Backend swap points

- `FrameViewModel.render_backend` is the backend selection hint transported from `saba_app`.
- `resolveRenderBackend(frame)` in `main.ts` is the single switch point for backend replacement.
- `RenderBackend.renderLeafFrame(...)` isolates leaf-frame rendering so `WebViewBackend` and future native backends can coexist.

## Current implementation status

- `WebViewBackend`: keeps compatibility path via `iframe srcdoc`.
- `NativeSceneBackend`: now renders `scene_items` directly into positioned DOM nodes (`rect`, `text`, `image`) without `iframe srcdoc`.
- The `frame.render_backend` field now selects between both implementations at runtime.
