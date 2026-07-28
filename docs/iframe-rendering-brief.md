# 専用セッション指示書: iframe の実ドキュメント描画（plan 2.7 の発展）

> 独立セッション用。着手前に `saba/HANDOFF.md` 全体と本書を読むこと。現状 iframe はプレースホルダ相当。**frameset の per-frame LivePage 基盤(`b550176`)が強い足場**。

## 背景 / 現在地

- plan 2.7 は iframe/video/canvas を **UA スタイル付きプレースホルダボックス**として描画する方針(埋め込み未対応)。実際 `<iframe>` 専用の描画/文書取得ロジックは無い。
- 一方、**frameset は実装済み**: セッションが frameset 文書をパースして子フレーム(各 `html_content` + rect)を持ち、`renderer_native/app_bridge.rs::AppBridge` が **フレームごとに `LivePage`** を保持してスクリプト実行+プログレッシブ描画する。plan D5 で複数 host が同一スレッドで安全に共存できる。
- ネットワーク取得は `cosmo_runtime/src/loader.rs`(`fetch_document` / `RuntimeFetchEngine`)。

## ゴール

`<iframe src>` を **ネストした browsing context** として: src 文書を取得し、iframe のボックス内にレイアウト・合成する。まずは同一/許可オリジンの静的〜軽 JS の iframe を対象。sandbox / cross-origin 制限は security.rs 経由。

## 難所

1. iframe は**インラインフロー内のボックス**(置換要素的)で、その中身は**別文書**。フレームツリー(session の frameset モデル)は「文書レベルの分割」だが iframe は「要素レベルの埋め込み」。両者のモデルをどう統一するか。
2. iframe の rect は親のレイアウト後に確定 → 子文書レイアウトは親レイアウト後の二段構え。
3. src の取得は非同期(loader)。初回はプレースホルダ、取得完了で差し込み → 既存の fetch waker / progressive 描画に載せられる。
4. sandbox 属性、cross-origin 分離、`window.parent`/postMessage の実接続(現状 postMessage はキャプチャのみ)。

## 推奨アプローチ（段階的）

1. **iframe ボックス確定**: レイアウトで `<iframe>` を width/height を持つ置換ボックスとして扱い、その rect を得る(現状の未知要素→box を専用化)。
2. **子文書の取得+ホスト**: iframe ごとに `LivePage` を生成(frameset の per-frame 機構を要素単位に一般化)。src を loader で取得 → HTML を LivePage::load、iframe rect をビューポートに。scene を iframe ボックス位置にオフセットして合成。
3. **合成**: iframe の scene_items を親 scene に(iframe の x/y でオフセット、clip)。`AppBridge` の per-frame 機構を再利用。
4. **非同期**: src 取得完了で waker → pump → 差し込み。sandbox/セキュリティは security.rs で判定。
5. **postMessage 実接続**(任意): 現状キャプチャの `window.parent.postMessage` を、親フレームの listener へ届ける。

## 検証

- `cd saba && cargo test`、reftest 12/12 維持(既存の iframe プレースホルダ挙動を壊さない)。
- ローカルサーバで「親 + iframe(別 HTML)」を配信し、iframe 内容が親内にオフセット描画されることをヘッドレススクショで確認。
- cross-origin iframe が security ポリシーで適切に制限される統合テスト。

## 撤退ライン

要素単位の埋め込み統合が重い場合、**同一オリジン・src 明示・JS 無しの iframe を静的に取得して合成**するところまで landing(sandbox/postMessage/cross-origin は追補)。video/canvas はプレースホルダ継続。

## 関連ファイル

- `saba/renderer_native/src/app_bridge.rs`(per-frame `LiveFrame` / splice / pump — 要素単位へ一般化)。
- `saba/cosmo_runtime/src/session.rs`(frameset/frame モデル)、`loader.rs`(`fetch_document`)。
- `saba/cosmo_engine/src/renderer/layout/`(iframe ボックス確定)、`dom/node.rs`(iframe 要素)。
- `saba/cosmo_script/src/lib.rs`(`window.parent.postMessage` 実接続)。
