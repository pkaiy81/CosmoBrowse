# HTTPS Display Implementation

## Purpose
- このドキュメントは、CosmoBrowse が HTTPS ページを取得・解析・描画する実装の現状をまとめたものです。
- Abe Hiroshi サイト（`https://abehiroshi.la.coocan.jp/`）を互換性ターゲットとして、取得・文字コード・frameset・ターゲット遷移の流れを整理します。

## Current Status
- ✅ HTTPS URL の取得は実装済みです（`reqwest` + Rust TLS スタック）。
- ✅ Shift_JIS を含む HTML デコードは実装済みです。
- ✅ `frameset/frame/noframes` の再帰解析とフレーム単位ナビゲーションは実装済みです。
- ✅ 描画は `scene_items` を使う Native Scene パイプラインが既定です。

## Code Map
- UI レンダリング/イベント: `saba/ui/cosmo-browse-ui/src/main.ts`
  - `openUrl()`
  - `handleEmbeddedNavigation()`
  - `renderFrame()`
  - `resolveRenderBackend()`
- Tauri IPC エントリ: `saba/ui/cosmo-browse-ui/src-tauri/src/lib.rs`
  - `dispatch_ipc()`
  - `open_url()`
  - `activate_link()`
- セッション/履歴/フレーム遷移: `saba/cosmo_app_legacy/src/session.rs`
  - `BrowserSession::open_url(...)`
  - `BrowserSession::activate_link(...)`
  - `load_page(...)`
  - `resolve_destination_frame_id(...)`
- ローダー/デコード/frameset 解析: `saba/cosmo_app_legacy/src/loader.rs`
  - `fetch_document(...)`
  - `decode_html_bytes(...)`
  - `resolve_url(...)`
  - `parse_frameset_document(...)`
  - `prepare_html_for_display(...)`

## End-to-End Flow
1. UI が URL を受け取り、Tauri IPC (`dispatch_ipc`) に `open_url` を送る。
2. セッション層が `fetch_document(...)` を呼び、HTTP/HTTPS を取得する。
3. レスポンス body を `decode_html_bytes(...)` で文字コード判定して UTF-8 文字列化する。
4. `parse_frameset_document(...)` で frameset を判定し、必要なら再帰的にフレームツリーを構築する。
5. 各フレームを `FrameViewModel + scene_items` に変換する。
6. UI 側が `NativeSceneBackend` で `scene_items` を描画する。
7. クリック時は `activate_link` を通じて Rust 側で target 解決し、必要なフレームだけ再読込する。

## HTTPS Fetching Details
- `fetch_document(...)` は共有 `reqwest::blocking::Client` を利用します。
- リダイレクトは最大回数を設けて追跡し、最終 URL を保持します。
- `ETag` / `If-None-Match` による再検証を行います。
- TLS・証明書・接続エラーは `classify_request_error(...)` で分類します。

## Charset Detection and Decoding
- 優先順:
  1. HTTP `Content-Type` の `charset=`
  2. `<meta charset>` / `http-equiv` ヒント
  3. UTF-8 フォールバック
- Shift_JIS 検出時の診断情報を保持します。

## Frameset and Targeted Navigation
- frameset 構造（`rows` / `cols`）を `FramesetSpec` として抽出します。
- フレームには安定した ID を割り当て、`target="right"` のような名前付きターゲットを解決します。
- `_self` / `_parent` / `_top` / `_blank` は ASCII 大文字小文字非依存で扱います。

## Rendering Notes
- 現在の既定描画は Native Scene で、`scene_items`（rect/text/image）を UI が直接描画します。
- `render_backend` が `web_view` でも現状の実装では Native Scene にフォールバックします。
- `prepare_html_for_display(...)` は互換目的で維持されていますが、主経路は scene 描画です。

## Verification
- 単体テスト:
  - `cargo test -p cosmo_app_legacy`
- CLI:
  - `cargo run -p adapter_cli --offline -- get-snapshot https://abehiroshi.la.coocan.jp/`
  - `cargo run -p adapter_cli --offline -- activate-link https://abehiroshi.la.coocan.jp/ root/left https://abehiroshi.la.coocan.jp/movie/eiga.htm right`
- UI 手動確認:
  - `npm run tauri dev` で起動し、Abe Hiroshi トップページを表示して左メニューから右フレーム更新を確認。

## Related Documents
- `docs/cosmobrowse-browser-architecture.md`
- `docs/architecture/runtime-topology.md`
- `docs/architecture/render-pipeline.md`
- `docs/architecture/integrated-sequence.md`
