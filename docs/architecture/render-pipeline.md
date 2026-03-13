# Render Pipeline

## Current (SceneItem native pipeline)

> Diagram source: `docs/architecture/mermaid/render-pipeline.mmd`

```mermaid
flowchart LR
  A["HTML LS parsing + DOM Standard tree"] --> B["CSS Display/CSS2 visual formatting model layout"]
  B --> C["Orbit Engine / Nebula Renderer tree"]
  C --> D["Starship Runtime DTO: GalaxyFrame + scene_items"]
  D --> E["cosmo-browse-ui RenderBackend resolver"]
  E --> F["NativeSceneBackend"]
  F --> G["SceneItem raster/compositor"]
```

## Stage responsibilities

```mermaid
flowchart TD
  S[Style\n- CSS Cascading/Inheritance\n- ComputedStyle defaults/inheritance] -->
  L[Layout\n- CSS2 visual formatting model\n- CSS Display block/inline tree\n- CSS Positioned Layout offsets]
  L --> P[Paint\n- DisplayItem generation\n- stacking context + z-index\n- clipping metadata]
  P --> C[Composite\n- SceneItem conversion\n- frame-space transform\n- diff friendly ordering]
```

## Legacy (deprecated WebView compatibility path)

```mermaid
flowchart LR
  A["Orbit Engine / Nebula Renderer tree"] --> B["Starship Runtime DTO: GalaxyFrame"]
  B --> C["cosmo-browse-ui RenderBackend resolver"]
  C --> D["WebViewBackend (deprecated)"]
  D --> E["iframe srcdoc"]
  E --> F["Platform WebView paints pixels"]
```

## Backend swap points

- `FrameViewModel.render_backend` is the backend selection hint transported from `cosmo_runtime`.
- `resolveRenderBackend(frame)` in `main.ts` is the single switch point for backend replacement.
- `RenderBackend.renderLeafFrame(...)` now resolves to scene item rendering only; WebView is deprecated.

## Current implementation status

- `NativeSceneBackend`: renders `scene_items` directly into positioned DOM nodes (`rect`, `text`, `image`) without `iframe srcdoc`.
- `WebViewBackend`: deprecated and no longer used for leaf-frame rendering in the UI pipeline.
- The effective runtime rendering path is scene-only, aligned with engine-produced paint order.
