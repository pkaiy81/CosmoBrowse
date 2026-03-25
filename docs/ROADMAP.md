# CosmoBrowse Roadmap v3 (WebView-free native rendering)

## 0. 結論（v3 方針）
- 最終目標は **WebView 非依存のネイティブ UI**（Chrome/Firefox のように専用描画スタックを持つ構成）。
- 移行期間は既存 Tauri UI を互換レイヤとして維持しつつ、標準 adapter を `adapter_native` に切り替える。
- Rust 資産（検索・ネットワーク・レンダリング）は `cosmo_core` / `cosmo_runtime` を中心に再利用する。
- 命名は段階的に CosmoBrowse へ移行する（`docs/cosmic-naming-migration.md`）。

---

## 1. 現状確認（2026-03-17 時点）
- Rust toolchain: `stable`（`saba/rust-toolchain.toml`）。
- Tauri (Rust): `tauri = "2"`, `tauri-build = "2"`, `tauri-plugin-opener = "2"`。
- Tauri (JS): `@tauri-apps/api = "^2"`, `@tauri-apps/cli = "^2"`。
- Frontend: `vite = "^6.0.3"`, `typescript = "~5.6.2"`。
- Tauri 側は `cosmo_runtime` 経由で `open_url` / `reload` / `back` / `forward` / `tabs` / `search` / `metrics` を接続済み。
- `RenderSnapshot` は本文・リンク・diagnostics に加えて layout/style メタ情報を返却。
- `adapter_cli` PoC を追加済みで、`open-url` / `get-snapshot` / `metrics` が同一 `cosmo_runtime` API で動作。
- `adapter_native` に Renderer 子プロセス管理（`ProcessHost`: spawn/kill/healthcheck/restart）を導入済み。
- Renderer 異常終了時は自動再起動し、`renderer_recovered` / `renderer_recovery_failed` を構造化ログで記録。
- 復旧中は retryable な `AppError`（`renderer_recovering`）を返却し、`cosmo_runtime` 経由で再試行可能にした。
- 障害注入テスト（強制終了→再起動→再試行成功）を `adapter_native` に追加済み。

---

## 2. アーキテクチャ v3（WebView 非依存を既定）

## 2.1 レイヤ
1. **Core (`cosmo_core`)**
   - Browser/Page/DOM/Parser/Renderer などのドメインロジック。
   - UI 依存禁止。
2. **Application (`cosmo_runtime` 新規)**
   - ユースケース API を提供（`open_url`, `navigate`, `search`, `tabs`）。
   - DTO（画面向けデータ）を定義し、Core の内部型を隠蔽。
3. **Adapter (`adapter_native`, 互換: `adapter_tauri`)**
   - UI フレームワーク固有処理（window/event/state binding）。
   - `cosmo_runtime` を呼ぶだけに限定。

## 2.2 依存ルール
- `adapter_* -> cosmo_runtime -> cosmo_core` の単方向のみ許可。
- UI が `cosmo_core` を直接 import するのは禁止。
- Core の内部構造体をフロントへ直接返さない（必ず DTO に変換）。

## 2.3 DTO の最小セット（v3）
- `NavigationState { can_back, can_forward, current_url }`
- `TabSummary { id, title, url, is_active }`
- `RenderSnapshot { title, text_blocks, links, diagnostics, layout, style }`
- `AppError { code, message, retryable }`
- `AppMetricsSnapshot { total_navigations, successful_navigations, failed_navigations, average_duration_ms, error_counts }`

---

## 3. 実装ロードマップ v3（10週間）

## Phase A: 土台再編（Week 1-2）
- [x] `cosmo_runtime` crate 新設。
- [x] workspace に UI/Tauri crate を正式参加。
- [x] `AppService`（同期/非同期 API）定義。
- [x] DTO / Error 体系定義。

**DoD**
- `cargo check --workspace` 成功。
- Tauri が `cosmo_runtime` 経由のみでコンパイル可能。

## Phase B: 最小ブラウズ体験（Week 3-4）
- [x] `open_url`, `reload`, `back`, `forward` 実装。
- [x] URL バー + ナビゲーションボタン + 本文表示 UI。
- [x] エラー表示（ネットワーク/パース失敗）。

**DoD**
- 主要3サイト程度で遷移と戻る/進むが動作。

## Phase C: タブ・検索統合（Week 5-6）
- [x] タブ API（`new/switch/close/list`）。
- [x] Rust 検索機能を `cosmo_runtime::search(query)` として統合。
- [x] 検索結果から遷移までの一連フローを接続。

**DoD**
- 複数タブ + 検索遷移がクラッシュなく動作。

## Phase D: 描画品質・観測性（Week 7-8）
- [x] `RenderSnapshot` を段階拡張（style/layoutメタ情報）。
- [x] ログ/メトリクス（遷移時間、失敗率、主要エラーコード）。
- [x] 代表ページのスモークテスト追加。

**DoD**
- 代表ページ群で表示成功率と失敗理由が取得可能。

## Phase E: WebView 依存除去（Week 9-10）
- [x] `adapter_native` を標準起動経路として有効化。
- [x] `adapter_tauri` をオプション機能に降格。
- [x] Renderer 子プロセス管理（spawn/kill/healthcheck/restart）を `adapter_native` に導入。
- [x] 異常終了時の自動再起動 + 構造化復旧ログ + retryable `AppError` 経路を実装。
- [x] 同一 `cosmo_runtime` API で動作確認（tauri/native の両経路）。

**DoD**
- 既定構成で WebView ランタイムなしに `open_url/get_snapshot` が動く。
- Renderer 障害注入時に復旧ログが残り、再試行で成功する。

---

## 4. バージョン運用 v3（見直し結果）

## 4.1 Rust
- 現状 `stable` は妥当。
- **v3 推奨**:
  - 開発中: `stable` 追従で速度優先。
  - ベータ前: `channel = "1.xx.0"` に pin（再現性重視）。
  - 全 crate に `rust-version` を記載して下限を固定。

## 4.2 Tauri
- 現状の major=2 は妥当。
- **v3 推奨**:
  - Rust 側は `"2"` のまま（互換性重視）。
  - JS 側は `^2` 維持、月次で lockfile 更新 + 動作確認。
  - リリース2週間前から dependency freeze（緊急修正のみ）。

## 4.3 Frontend
- `typescript ~5.6.2` は堅実。
- `vite ^6.0.3` は minor 更新を取り込みつつ、月次で回帰確認。

## 4.4 更新ルール
- 月1回: `cargo update` / `npm update`。
- CI: `cargo check --workspace`, `cargo test --workspace`, `npm run build`。
- 依存更新PRは自動化（Dependabot or Renovate）。

---

## 5. 直近スプリント（次の2週間でやること）
1. [x] `cosmo_runtime` crate を追加。
2. [x] `AppService` trait + 実装を作成。
3. [x] `open_url` と `get_render_snapshot` を `cosmo_runtime` に実装。
4. [x] Tauri command は上記2つのみ先に接続。
5. [x] フロントに URL バー + 読み込みボタン + 表示領域を実装。
6. [x] エラーDTOを画面表示。
7. [x] CIに workspace check/test + frontend build を追加。

**2週間の完了基準**
- URL入力から本文表示まで一気通貫で動作。
- UI 直結ロジックが `cosmo_runtime` の外に漏れていない。

---

## 6. リスクと対策（v3）
- **Tauri command 肥大化**
  - 対策: command は引数検証 + `cosmo_runtime` 呼び出しのみ。
- **Core 型漏れでUI依存固定化**
  - 対策: DTO境界を厳守、レビュー項目化。
- **依存更新の破壊的影響**
  - 対策: 月次更新 + スモークテスト + freeze期間設定。

---

## 7. KPI（v3）
- ナビゲーション成功率
- 平均ページ表示完了時間
- コマンド失敗率（`open_url`, `search` など）
- クラッシュ率
- 別adapterで再利用できた API 比率


---

## 8. IPC adapter migration plan（invoke互換維持）
1. `adapter_native` を Browser Process の標準 adapter として導入し、`IpcRequest` / `IpcResponse` を IPC 契約として固定する。
2. `ui/cosmo-browse-ui/src-tauri` の command 群は `adapter_tauri` 互換レイヤとして維持し、内部では `adapter_native` を呼び出す。
3. `adapter_native` では Renderer プロセスの healthcheck/restart を常時実施し、復旧イベントを構造化ログに記録する。
4. 復旧中は retryable な `AppError`（`renderer_recovering`）を返し、フロントは再試行 UI へフォールバックする。
5. デフォルト実行経路は `dispatch_ipc`（schema-based）へ切替える。
6. 既存フロントは段階移行:
   - Step 1: 既存 `invoke("open_url")` 等を維持（互換モード）。
   - Step 2: 新規実装は `invoke("dispatch_ipc", { request })` を使用。
   - Step 3: 移行完了後に互換 command の利用状況を監視し、削除可否を判断する。
7. 移行期間のレビュー観点:
   - `adapter_* -> cosmo_runtime -> cosmo_core` 依存方向を維持。
   - DTO/IPC 契約に `tauri` 固有型を混入させない。

---

## 9. Chrome/Firefox 相当を目指す実行タスク（生成版）

> 目的: 「WebView 依存アプリ」から「独自描画スタックを持つブラウザ」へ段階移行するため、実装・検証可能な粒度でタスク化する。

### Epic 1: マルチプロセス基盤（Browser/Renderer/Network 分離）
- [x] **E1-T1 プロセス責務定義を ADR 化**
  - Browser/Renderer/Network/GPU(将来) の責務、障害時の復旧方針、IPC 境界を明文化。
  - **完了条件**: ADR が `docs/architecture/` に追加され、レビューで承認済み。
- [x] **E1-T2 `adapter_native` に ProcessHost を実装**
  - Renderer 子プロセスの起動・停止・ヘルスチェック API を追加。
  - **完了条件**: 異常終了時に自動再起動し、復旧ログが残る。
  - 進捗メモ: `ProcessHost`（spawn/kill/healthcheck/restart）と fault injection テストを導入し、`renderer_recovering` の再試行経路まで接続済み。
- [x] **E1-T3 IPC 契約を schema 固定**
  - `IpcRequest/IpcResponse` を version 付きで定義し、後方互換ルールを策定。
  - **完了条件**: schema 変更時に CI で破壊的変更を検知可能。
  - 進捗メモ: schema 差分チェックを CI に導入し、破壊的変更を PR で fail-fast 化。

### Epic 2: ナビゲーションエンジン強化（Chrome/Firefox 的な遷移品質）
- [x] **E2-T1 履歴モデルを document 単位へ拡張**
  - same-document/hash 遷移、redirect chain を区別して履歴管理。
  - **完了条件**: back/forward の挙動が主要ケースで期待どおり。
  - 進捗メモ: redirect chain を履歴単位で正規化し、hash 遷移との競合ケースを回帰テスト化。
- [x] **E2-T2 セッション復元（クラッシュ復旧）**
  - タブ/履歴/スクロール位置をスナップショット保存。
  - **完了条件**: 異常終了後の再起動で前回セッションが復元される。
  - 進捗メモ: `SessionSnapshot` schema を `cosmo_runtime` 境界へ追加し、active tab・履歴スタック・frame URL override・scroll position を保存/復元、adapter/UI の restore smoke まで接続済み。
- [x] **E2-T3 ナビゲーションガード**
  - 無効 URL・危険スキーム・無限リダイレクトの検知と `AppError` 整備。
  - **完了条件**: エラーコードが UI に統一表示される。
  - 進捗メモ: `navigation_guard_blocked` を追加し、危険スキーム（`mailto/javascript/data/file`）およびリダイレクト上限超過を同一エラー系で返却。

### Epic 3: 描画パイプライン（レイアウト/ペイント/コンポジット）
- [x] **E3-T1 RenderTree DTO を段階導入**
  - `RenderSnapshot` に加え、ボックス情報・スタイル解決済みノードを返却。
  - **完了条件**: テキスト/リンク以外に block/inline 構造を UI で描画可能。
  - 進捗メモ: RenderTree DTO を `cosmo_runtime` 境界で固定し、tauri/native 両経路で同一 payload を再生確認。
- [x] **E3-T2 最小レイアウトエンジン v1**
  - normal flow（block/inline）、margin/padding/border の計算を実装。
  - **完了条件**: 代表ページで要素重なり崩れ率を計測し、基準値以下。
  - 進捗メモ: `LayoutView` ベースの normal flow と box model 計算を `RenderTreeSnapshot` へ反映し、`layout-regression-policy` と smoke 集計で崩れ率を継続計測できる状態にした。
- [x] **E3-T3 ペイントコマンド生成**
  - `DrawRect/DrawText/DrawImage` コマンド列を生成し、backend 非依存化。
  - **完了条件**: 同一コマンド列を native/tauri 互換で再生可能。
  - 進捗メモ: `scene_items_to_paint_commands` と paint snapshot test を導入し、native/tauri 双方の UI で同じ paint command を再生する経路を固定した。

### Epic 4: Web Platform 機能（最低限の実利用ライン）
- [x] **E4-T1 JavaScript 実行基盤の統合**
  - JS runtime を導入し、DOM 読み取り/イベント dispatch まで接続。
  - **完了条件**: 静的サイト + 軽量 SPA の初期描画が完了する。
  - 進捗メモ: `build_layout_scene_with_script_runtime` に `JsRuntime` を統合し、DOM 更新後の relayout/repaint と lightweight SPA smoke を接続済み。
- [x] **E4-T2 イベントループ/タスクキュー実装**
  - microtask/macrotask の実行順序を定義し、タイマー API を追加。
  - **完了条件**: Promise/timeout を使うページでハングしない。
  - 進捗メモ: `setTimeout` / `queueMicrotask` / `Promise.then` と guard 付き event loop を実装し、`adapter_cli verify-event-loop` と単体テストで順序とハング検知を検証可能にした。
- [x] **E4-T3 ストレージ/クッキー/権限モデル**
  - origin 単位で cookie/local storage を管理。
  - 進捗メモ: `PersistentSecurityState` を導入し、`Set-Cookie` 適用・`Cookie` header 送信・`localStorage` JS 連携・権限判断キャッシュ・ADR 追記まで接続済み。
  - **完了条件**: ログイン状態を持つサイトで再訪時セッション維持。

### Epic 5: ネットワーク/セキュリティ
- [x] **E5-T1 HTTP スタック拡張**
  - redirect policy、cache-control、content-encoding、timeout 制御を統合。
  - **完了条件**: 計測対象サイトで失敗率と表示時間を改善。
  - 進捗メモ: `reqwest` loader に redirect 上限、`ETag`/`Cache-Control` 再検証、`Content-Encoding` 診断、timeout/error code 分類を統合した。
- [x] **E5-T2 Same-Origin/CORS の導入**
  - Fetch/XHR ポリシー評価と preflight を実装。
  - **完了条件**: セキュリティテストで CORS 回避が不可。
  - 進捗メモ: `evaluate_cors_request` で same-origin 判定・credentials 制約・preflight 検証を実装し、拒否ケースの回帰テストを追加した。
- [x] **E5-T3 TLS/証明書エラー UX**
  - 証明書期限切れ・自己署名の警告画面と例外登録。
  - **完了条件**: 危険接続時に必ず interstitial が表示される。
  - 進捗メモ: TLS エラー分類、origin/TTL 付き例外登録、UI interstitial と再試行フローを `dispatch_ipc` 経路へ接続済み。

### Epic 6: UI/UX（タブ・アドレスバー・開発者体験）
- [x] **E6-T1 タブストリップ改善**
  - ピン留め、ミュート、複製、ドラッグ並べ替え。
  - **完了条件**: 20 タブ以上で操作遅延が許容値内。
- 進捗メモ: タブごとの `pin/mute/duplicate/reorder` API と UI を接続し、session snapshot で pin/mute を保持、`scripts/benchmark_tab_strip.mjs` で 20 タブの軽量遅延計測を追加。
- [x] **E6-T2 アドレスバー（Omnibox）**
  - URL/履歴/検索候補を統合し、キーボード操作を最適化。
  - **完了条件**: 主要ショートカットで遷移完了まで一貫動作。
- 進捗メモ: `OmniboxSuggestionSet` DTO に履歴 + 既存検索候補 + active index を統合し、Enter / ArrowUp / ArrowDown / Escape / Ctrl(Cmd)+L / Alt+Left/Right を UI へ実装。
- [ ] **E6-T3 ダウンロードマネージャ**
  - 実装済み機能（2026-03-24 時点）:
    - `enqueue/list/progress/pause/resume/cancel/open/reveal` の IPC 契約と UI 操作線を接続済み。
    - `saba/cosmo_app_legacy/src/download.rs` で cooperative pause/resume（分割受信ループでの中断・再開）を実装済み。
  - 残課題（未達）:
    - HTTP Range ベースの厳密 resume 検証（`Accept-Ranges`/`Content-Range` 整合、ETag/Last-Modified 再検証を含む）。
    - 保存先ポリシー（毎回確認/既定フォルダ/サイト別方針）の UI 設定導線。
    - 大容量ファイル回帰スモーク（失敗時リトライ + 完了ファイル整合性検証）の定常運用化。
  - **完了条件（DoD）**:
    - 上記残課題を解消し、ダウンロード検証ポリシー文書（`docs/layout-regression-policy.md` と同等粒度）で測定方法を固定したうえで、
      大容量ファイルの pause/resume/retry を含む CI スモークが連続成功すること。

### Epic 7: 観測性・品質保証
- [x] **E7-T1 ブラウザ KPI ダッシュボード**
  - 成功率、クラッシュ率、FCP 相当、メモリ使用量を定期収集。
  - **完了条件**: CI/ nightly で時系列比較が可能。
  - 進捗メモ: `kpi_summary.json` を拡張し、`fcp_equivalent_ms` / `memory_usage_kib` / ケース別失敗分類を nightly artifact の `history/<history_key>/` に集約、`evaluate_release_gate.py` / `check_release_streak.py` へ履歴キーを付与して時系列比較しやすくした。
- [x] **E7-T2 Web 互換スモークスイート**
  - 代表サイト群のナビゲーション/描画/入力を自動検証。
  - **完了条件**: リグレッションを PR 時点で検知。
  - 進捗メモ: PR 軽量版 + nightly 拡張版の二段構成で導入し、失敗時ログを artifact 保存。
- [x] **E7-T3 クラッシュレポート基盤**
  - panic/minidump 収集、シンボル管理、再現手順テンプレート化。
  - **完了条件**: まず nightly artifact 集約・再現手順テンプレート自動添付・クラッシュ分類ダッシュボードを完了し、その後 minidump/symbol 管理へ段階拡張する。
  - 進捗メモ: panic report に build id / commit hash / transport / active URL / last command を保存し、履歴付き crash repository から最新 report を取得、nightly artifact へ `latest_crash_report.json` を添付して crash 分類を KPI ダッシュボードへ反映した。

### Epic 8: リリース/移行完遂（WebView-free デフォルト化）
- [x] **E8-T1 `adapter_native` をデフォルト起動へ切替**
  - feature flag で段階ロールアウトし、失敗時に互換経路へフォールバック。
  - **完了条件**: 安定チャネルで WebView なし既定起動を達成。
- [x] **E8-T2 `adapter_tauri` の互換範囲を最小化**
  - 旧 command 利用統計を取り、未使用 API を削減。
  - **完了条件**: 互換レイヤの保守コストを定量的に削減。
- [x] **E8-T3 GA 判定ゲート運用**
  - 性能・安定性・互換性の出荷判定基準を運用へ組み込み。
  - **完了条件**: GA 判定を満たすまで自動的に release をブロック。

### 実行順序（推奨）
1. Epic 1 → 2 → 3 で「ブラウザ骨格」を先に完成。
2. Epic 4/5 を並行実施して「実サイト互換性」を引き上げ。
3. Epic 6/7 で「日常利用品質」と「継続改善ループ」を確立。
4. Epic 8 で段階ロールアウトし、WebView-free を正式化。


## 10. 完成までの新規タスク（再計画: 2026-03-17）

> 前回計画（E1-T1/E1-T3/E2-T1/E3-T1/E7-T2）完了を前提に、GA 到達までの残作業を再編。

### Sprint F（次の2週間）: レイアウト/ペイントの実用化
1. [x] **NF-T1 E3-T2 最小レイアウトエンジン v1 を実装**
   - block/inline の normal flow、margin/padding/border の計算を本実装へ。
   - 代表ページ群で「要素重なり崩れ率」を定量化し、基準値をドキュメント化。
2. [x] **NF-T2 E3-T3 ペイントコマンド backend 非依存化**
   - `DrawRect/DrawText/DrawImage` を標準描画コマンドとして固定。
   - tauri/native 双方で同一コマンド列再生テストを CI に追加。
3. [x] **NF-T3 描画回帰テスト拡張**
   - RenderTree と paint command の snapshot テストを導入し、差分を可視化。

### Sprint G（続く2週間）: Web Platform 最低ライン
4. [x] **NG-T1 E4-T1 JavaScript 実行基盤を統合**
   - DOM 読み取り・イベント dispatch を最小接続し、軽量 SPA 初期描画を成立。
5. [x] **NG-T2 E4-T2 イベントループ/タスクキュー実装**
   - microtask/macrotask の順序を仕様化し、Promise/timeout のハングを解消。
6. [x] **NG-T3 E4-T3 ストレージ/クッキー基盤（Sprint J の NJ-T2 へ統合済み）**
   - 同一成果物のため完了履歴は NJ-T2 に集約（重複管理を解消）。

### Sprint H1（2週間）: HTTP 信頼性基盤 + 失敗理由統一
7. [x] **NH1-T1 E5-T1 HTTP スタック統合フェーズ 1**
   - redirect policy / cache-control / content-encoding / timeout を Loader に統合。
   - 失敗理由を `AppError` コードに統一マッピングし、UI では単一のエラー描画経路へ収束。
   - 失敗率 KPI（`open_url` / `search`）を nightly 収集へ追加し、導入前後を時系列比較可能にする。
   - 仕様準拠メモ:
     - Redirect は RFC 9110 Section 15.4（3xx）と RFC 9111 Section 4（キャッシュキー/再利用条件）に合わせ、上限回数超過時は deterministic に `AppError::NetworkRedirectLoop` へマップする。
     - `Cache-Control` は RFC 9111 Section 5.2（`no-store` / `max-age`）を最低限実装し、再検証が必要な応答は stale 利用を禁止する。
     - `Content-Encoding` は RFC 9110 Section 8.4 に基づき、`gzip` / `deflate` 展開失敗を transport 成功扱いせず `AppError::NetworkContentDecoding` として明示する。
     - Timeout は RFC 9110 Section 9.2.2（冪等メソッドの再試行）を前提に `GET` 系のみ自動リトライ可とし、非冪等メソッドは即失敗する。

### Sprint H2（2週間）: CORS 強制 + 回避不能テスト
8. [x] **NH2-T1 E5-T2 Same-Origin/CORS 実装フェーズ 2**
   - Fetch/XHR の Same-Origin 判定、CORS ヘッダー評価、preflight（`OPTIONS`）を実装。
   - 回避不能な検証ケース（opaque response / wildcard + credentials 禁止 / preflight 失敗）を自動テストに追加する。
   - H1 で追加した `AppError` taxonomy に CORS 系コード（`CorsPreflightFailed` など）を接続し、UI へ同一フォーマットで表示する。
   - 仕様準拠メモ:
     - Origin 判定は RFC 6454 Section 4（scheme/host/port tuple）で実施し、tuple 不一致は cross-origin とみなす。
     - CORS 判定は Fetch Standard「HTTP fetch」「CORS-preflight fetch」の手順に合わせ、`Access-Control-Allow-Origin` と `credentials mode` の組み合わせを厳密評価する。
     - XHR のエラー可視化は WHATWG XHR Standard（network error / CORS error）に準拠し、JavaScript には詳細を漏らさず UI 診断ログにのみ原因を残す。

### Sprint H3（2週間）: 危険接続 interstitial + 監視定着
9. [x] **NH3-T1 E5-T3 TLS/証明書エラー UX フェーズ 3**
   - 証明書エラー時に interstitial を必ず表示し、ユーザー明示操作による例外登録フローを実装。
   - 危険接続検知率 KPI（証明書エラー検知数 / 危険接続試行数）を nightly 継続監視へ追加。
   - 表示時間 KPI（interstitial 表示までの時間、例外登録後の再遷移時間）を計測し、失敗率改善と同時に追跡する。
   - 仕様準拠メモ:
     - 証明書エラー分類は RFC 5280（有効期限/発行者/パス検証）に合わせ、原因別 `AppError` を固定コード化する。
     - HTTPS 失敗時の警告 UX は Chromium SSL interstitial の設計原則（ユーザーに危険性を明示し、デフォルトは接続継続不可）を踏襲し、例外登録は 1) 明示操作 2) 期限付き 3) Origin 単位に制限する。
     - HSTS 対象相当の接続では例外登録を禁止し、危険接続保護を優先する（RFC 6797 の fail-closed 方針）。


### Sprint J（次の再計画候補）: セッション復元・UX・運用ギャップ解消
10. [x] **NJ-T1 E2-T2 セッション復元（クラッシュ復旧）**
    - タブ一覧・履歴スタック・スクロール位置をクラッシュセーフなスナップショットへ保存し、起動時に restore する。
    - 完了時には `renderer_recovered` 後の再起動経路でも、最後の active tab が自動復元されることをスモークで保証する。
    - 進捗メモ: `cosmo_app_legacy` / `adapter_native` / UI 間で session snapshot restore を実装し、session round-trip test と startup restore smoke で active tab・history_index・frame scroll の復元を検証済み。
11. [x] **NJ-T2 E4-T3 origin 単位ストレージ/クッキー永続化**
    - Cookie jar / local storage / 権限判断キャッシュを origin 単位で保存し、SameSite/Secure/HttpOnly 診断と接続する。
    - 完了日: **2026-03-17**（E4-T3 完了履歴の単一記録点）。
    - 実装参照:
      - 画面/セッション経路: `saba/cosmo_app_legacy/src/session.rs`
      - ダウンロード機能との整合確認: `saba/cosmo_app_legacy/src/download.rs`
      - 永続化実体（`PersistentSecurityState`）: `saba/cosmo_app_legacy/src/security.rs`
      - Cookie 入出力配線: `saba/cosmo_app_legacy/src/loader.rs`
      - `localStorage` ランタイム同期: `saba/cosmo_app_legacy/src/layout/mod.rs`
      - 永続化方針 ADR: `docs/architecture/adr-0002-origin-storage-persistence.md`
    - 検証経路（集約）:
      - unit test: `cargo test -p cosmo_app_legacy security::tests::set_cookie_updates_origin_scoped_jar_and_request_header`
      - unit test: `cargo test -p cosmo_app_legacy security::tests::insecure_same_site_none_cookie_is_rejected_and_not_stored`
      - unit test: `cargo test -p cosmo_app_legacy security::tests::local_storage_snapshots_are_origin_scoped`
    - 進捗メモ: RFC 6454 origin key を使う in-memory 永続モデルと `localStorage` ランタイム同期、Cookie 診断/送信経路、永続化形式 ADR を追加。
    - セッション復元と干渉しないよう、永続化形式と消去ポリシーを ADR 化する。
12. [x] **NJ-T3 E6-T1/E6-T2 タブストリップ + Omnibox 改善**
    - ピン留め/複製/並べ替えと URL・履歴・検索候補を統合した入力補完を導入する。
    - 20 タブ規模の UI 操作遅延と主要ショートカット回帰を Playwright か smoke harness で定量評価する。
    - 進捗メモ: runtime 境界へ `OmniboxSuggestionSet` を追加し、タブストリップ操作とショートカットを `saba/ui/cosmo-browse-ui` に接続、`scripts/benchmark_tab_strip.mjs` で 20 タブ render p95 / interaction batch を smoke 計測可能にした。
13. [x] **NJ-T4 E7-T1/E7-T3 KPI ダッシュボード + クラッシュレポート強化**
    - 既存 `kpi_summary.json` を拡張して FCP 相当・メモリ使用量・クラッシュ分類を時系列可視化し、panic report を release artifacts と紐付ける。
    - minidump/symbol 管理までは到達していないため、まず nightly artifact 集約と再現テンプレート自動添付を完了条件にする。
    - 進捗メモ: nightly workflow が `smoke-kpi-history-nightly` artifact を生成し、`ga-gate-report.json` / `kpi_summary.json` / `latest_crash_report.json` を同じ `history_key` で束ねるようにした。

### Sprint I（GA前）: デフォルト切替・運用完成
14. [x] **NI-T1 E8-T1 native デフォルト起動の段階ロールアウト**
    - feature flag とフォールバックを運用し、安定チャネルへ展開。
15. [x] **NI-T2 E8-T2 互換 command の利用統計/削減**
    - `adapter_tauri` の未使用 API を削減し、保守対象を最小化。
16. [x] **NI-T3 E8-T3 GA 判定ゲート運用**
    - 性能・安定性・互換性しきい値を CI/release pipeline に強制適用。

### 完了基準（更新）
- [ ] WebView なし既定起動で、主要シナリオ（遷移/戻る進む/検索/タブ/軽量SPA）が連続成功する。
- [ ] KPI（成功率・クラッシュ率・平均表示時間・FCP 相当・メモリ使用量）が GA しきい値を連続で満たす。
  - 判定成果物キー（`history/<history_key>/kpi_summary.json`）:
    - `success_rate >= 0.99`（= `failure_rate <= 0.01`）
    - `crash_rate <= 0.005`
    - `display_time_ms <= 1500`
    - `fcp_equivalent_ms` が直近 3 回中央値比 `+10%` 以内
    - `memory_usage_kib.combined_peak_rss_kib` が直近 3 回中央値比 `+15%` 以内
  - 連続達成条件:
    - `history/<history_key>/ga-gate-report.json` の `consecutive_pass_streak >= 3`
    - 同ファイルの `required_consecutive_passes == 3` と整合し、`release_blocked == false`
- [ ] `ga-gate-report.json` の mandatory gate が pass している。
  - 判定成果物キー（`history/<history_key>/ga-gate-report.json`）:
    - `gate_passed == true`
    - `checks[name=success_rate].passed == true`
    - `checks[name=crash_rate].passed == true`
    - `checks[name=display_time_ms].passed == true`
    - `checks[name=layout_regression].passed == true`
    - `checks[name=download_regression].passed == true`
    - `checks[name=crash_exception_metadata].passed == true`
- [ ] 重大クラッシュ閾値を下回っている。
  - 判定成果物キー（`history/<history_key>/latest_crash_report.json` + `history/<history_key>/kpi_summary.json`）:
    - `kpi_summary.json.failure_classification.crash.count == 0` を原則条件とする
    - 例外的に `count == 1` の場合は `latest_crash_report.json` に `transport` / `active_url` / `last_command` / `build_id` / `commit_hash` が揃い、単発で再現手順付きであること
    - 同一 `failure_classification.crash.categories[*]` が 2 nightlies 連続した場合は未達（ロールバック判定）
- [ ] `adapter_tauri` は互換層として最小維持され、コア機能追加は `adapter_native` のみで成立する。


## 11. 進捗棚卸し（2026-03-24）と追加タスク

### 11.1 棚卸し結果
- [x] Epic 1〜5 は完了。
- [x] Epic 6 は E6-T1/E6-T2 完了。
- [ ] Epic 6 の E6-T3（ダウンロードマネージャ最終化）は未完了。
- [x] Epic 7〜8 は実装完了（運用は継続監視中）。
- [ ] GA 完成判定は `consecutive_pass_streak >= 3` の連続達成が未確認。

### 11.2 E6-T3 完了に向けた追加タスク（DL series）
17. [x] **DL-T1 HTTP Range strict resume 検証**
    - 206 応答時の `Content-Range` と保存済みオフセットの一致を必須化する。
    - `ETag` / `Last-Modified` 再検証不一致時は resume せず full restart へフォールバックする。
    - 進捗メモ（2026-03-24）: `saba/cosmo_app_legacy/src/download.rs` で `Content-Range` 開始オフセット検証と validator mismatch fallback を実装し、回帰テストを追加済み。
    - 仕様準拠メモ:
      - HTTP range request/resume は RFC 9110 Section 14（Range Requests）に準拠し、range satisfiable 条件をサーバー応答で検証する。
      - キャッシュ再検証は RFC 9111 Section 4.3（Validation）に合わせ、validator 不一致時の部分再利用を禁止する。
18. [x] **DL-T2 保存先ポリシー UI/永続化**
    - 保存先ポリシー（毎回確認/既定フォルダ/サイト別）を UI 設定に追加。
    - policy は session restore 後も維持し、origin 単位の例外設定を監査ログに記録する。
    - 進捗メモ（2026-03-24）: `get/set/clear_download_*policy` の IPC 契約と UI 設定フォームを追加し、`SessionSnapshot.download_policy_settings` を使って restore 後も default/site policy を復元するようにした。
19. [x] **DL-T3 大容量回帰スモーク定常化**
    - pause/resume/retry を含む大容量 fixture を nightly へ追加。
    - 完了ファイルは checksum（SHA-256）一致を必須判定にし、失敗時は crash report と同一 history_key に束ねる。
    - 進捗メモ（2026-03-24）: `scripts/run_download_regression.py` を追加し、`pause_resume_checksum` + `retry_after_cancel` を nightly で検証、`download_regression_summary.json` を KPI history artifact に同梱するよう workflow を更新した。
20. [x] **DL-T4 ダウンロード検証ポリシー文書化**
    - `docs/layout-regression-policy.md` と同等粒度で、ダウンロード回帰の測定・失敗分類・許容閾値を文書化する。
    - nightly と PR 軽量版の差分（fixture サイズ、再試行回数、タイムアウト）を明示し、運用手順を固定する。
    - 進捗メモ（2026-03-24）: `docs/download-regression-policy.md` を追加し、PR placeholder / nightly 本検証の実行条件と失敗分類を固定した。

### 11.3 GA 判定運用タスク（連続達成の最終化）
21. [x] **GA-T1 3連続 pass の自動証跡化**
    - `ga-gate-report.json.consecutive_pass_streak` と `required_consecutive_passes` を nightly ごとに保存し、3連続達成時に release unblock を自動判定する。
    - 進捗メモ（2026-03-25）: nightly workflow に `check_release_streak.py` を追加し、`release-streak-report.json` を artifact 化して 3 連続判定の証跡を自動生成するようにした。
22. [x] **GA-T2 crash 単発例外の運用ルール固定**
    - `crash.count == 1` を許容する際の再現テンプレート必須項目（transport/active_url/last_command/build_id/commit_hash）をチェックリスト化し、CI で欠落を fail にする。
    - 進捗メモ（2026-03-25）: `evaluate_release_gate.py` に `crash_exception_metadata` mandatory check を追加し、`latest_crash_report.json` 欠落項目（transport/active_url/last_command/build_id/commit_hash）を fail-fast するようにした。
23. [x] **GA-T3 release unblock 証跡の README 連携**
    - `history/<history_key>/ga-gate-report.json` の最終 pass 証跡を README から辿れる運用（最新 artifact 参照先）を追加する。
    - ローカル実行者が「未判定」か「達成済み」かを 1 画面で判定できる導線を整備する。
    - 進捗メモ（2026-03-24）: README に `smoke-kpi-history-nightly` artifact 参照導線と `ga-gate-report.json` / `kpi_summary.json` / `download_regression_summary.json` / `latest_crash_report.json` の判定キーを追記した。

### 11.4 今回棚卸しでの結論（2026-03-25 更新）
- [x] 実装面の主な未完了だった E6-T3 は DL-T3/DL-T4 実装で完了。
- [x] GA 判定運用タスク（GA-T1/GA-T2/GA-T3）は実装完了。
- [ ] 残課題は「3 連続 pass 実績を artifact 上で満たすこと」のみ。
- [ ] ローカル smoke 実測は環境制約で未完了（`rustup` の stable チャネル取得に失敗し `native_ipc_cli` build が失敗）。
