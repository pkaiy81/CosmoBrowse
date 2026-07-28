# ADR-0003: Engine Consolidation and std Migration

- Status: Accepted
- Date: 2026-07-13
- Deciders: CosmoBrowse maintainers
- Related: `docs/modernization-plan.md`, `docs/cosmic-naming-migration.md`, ADR-0001

## Context

モダンブラウザ化計画（`docs/modernization-plan.md`）の Phase 0 として、レガシー構成を整理した。従来は saba 本由来の `#![no_std]` エンジン（`cosmo_core_legacy`）の上に再エクスポート用シム（`cosmo_core`）とラッパー（`cosmo_runtime` → `cosmo_app_legacy`）が重なり、コズミック系モジュール別名（`orbit_engine` / `nebula_renderer` / `stardust_display` 等）が第二の命名層を形成していた。また Wasabi OS ターゲット・Tauri シェル・wasm レンダラは全てワークスペースから除外済みで実質死んでいた。

## Decision

1. **no_std の廃止**: エンジンは std を前提とする。Wasabi OS ターゲット（`saba/src`、`net/wasabi`、`ui/wasabi`）は削除。これによりエコシステムのクレート（フォント解析・Unicode データ表・圧縮等のデータ/デコード系）を採用可能になる。ただし**レンダリングエンジンのロジック（HTML パース・CSS・レイアウト・ペイント）は自作を継続**する（html5ever/stylo/taffy 等は使わない）。
2. **クレート統合**: `cosmo_core_legacy` → `cosmo_engine`（シム `cosmo_core` の実体モジュール `paint_commands` / `paint_mapper` / `js_runtime` を吸収）。`cosmo_app_legacy` → `cosmo_runtime`（旧ラッパーを解消、`scene_items_to_paint_commands` は `cosmo_runtime::paint` へ）。
3. **平易な命名に統一**: コズミック系の別名は全廃。`cosmo_engine::{browser, renderer, display_item}`、`BrowserApp` / `PageViewModel` / `FrameViewModel` を正とする。
4. **削除**: Tauri シェル（`ui/cosmo-browse-ui`）と `renderer-wasm` は削除（git 履歴から復元可能）。ネイティブ winit スタック（`renderer_native` → `adapter_native` → `cosmo_runtime` → `cosmo_engine`）に一本化。
5. **エンジンの HTTP/URL モジュール削除**: `http.rs` / `url.rs`（Wasabi 時代の手書き実装）は削除。ネットワークはアプリ層（reqwest + url クレート）の責務。`Page::receive_response(HttpResponse, ..)` は `Page::load_html(html, ..)` に変更。

## Consequences

- CI は `saba_*` / `*_legacy` / `cosmo_core` への参照をエラーにする（`.github/workflows/ci.yml`）。
- ADR-0001 のプロセスモデル・IPC 境界は不変（Phase 5 で実現予定）。依存方向は `adapter_* → cosmo_runtime → cosmo_engine` の一方向を維持。
- Windows ポータブル配布（Tauri ベース）は一旦停止。ネイティブバイナリの配布は将来フェーズで再設計。
