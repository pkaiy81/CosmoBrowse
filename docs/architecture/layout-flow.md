# Layout flow

CosmoBrowse の最小レイアウトパイプライン（style → layout → paint）と、frameset 矩形への接続点。

```mermaid
sequenceDiagram
    participant Session as saba_app::BrowserSession
    participant Frame as Frame loader (frameset/leaf)
    participant Style as saba_core::CSS parser
    participant Layout as saba_core::layout::LayoutView
    participant Paint as DisplayItem -> SceneItem

    Session->>Frame: load_page(url, viewport, overrides)
    Frame->>Frame: frameset child_rects(FrameRect)
    Frame->>Style: parse <style> + computed style inputs
    Style->>Layout: build layout tree (DOM tree order)
    Layout->>Layout: block/inline/positioned minimal layout
    Layout->>Paint: paint() => DisplayItem[]
    Paint->>Session: map to SceneItem[] with frame rect offset

    Note over Session,Layout: Relayout trigger 1: viewport changed
    Note over Session,Layout: Relayout trigger 2: DOM changed (navigation/reload)
```

- Frameset 文書では `FrameRect` で子フレーム領域を分割し、leaf 文書で style/layout/paint を実行する。
- leaf 文書の `SceneItem` 座標は `FrameRect` の `(x, y)` を加算してページ座標へ変換する。
