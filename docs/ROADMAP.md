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
- [ ] **E2-T2 セッション復元（クラッシュ復旧）**
  - タブ/履歴/スクロール位置をスナップショット保存。
  - **完了条件**: 異常終了後の再起動で前回セッションが復元される。
- [x] **E2-T3 ナビゲーションガード**
  - 無効 URL・危険スキーム・無限リダイレクトの検知と `AppError` 整備。
  - **完了条件**: エラーコードが UI に統一表示される。
  - 進捗メモ: `navigation_guard_blocked` を追加し、危険スキーム（`mailto/javascript/data/file`）およびリダイレクト上限超過を同一エラー系で返却。

### Epic 3: 描画パイプライン（レイアウト/ペイント/コンポジット）
- [x] **E3-T1 RenderTree DTO を段階導入**
  - `RenderSnapshot` に加え、ボックス情報・スタイル解決済みノードを返却。
  - **完了条件**: テキスト/リンク以外に block/inline 構造を UI で描画可能。
  - 進捗メモ: RenderTree DTO を `cosmo_runtime` 境界で固定し、tauri/native 両経路で同一 payload を再生確認。
- [ ] **E3-T2 最小レイアウトエンジン v1**
  - normal flow（block/inline）、margin/padding/border の計算を実装。
  - **完了条件**: 代表ページで要素重なり崩れ率を計測し、基準値以下。
- [ ] **E3-T3 ペイントコマンド生成**
  - `DrawRect/DrawText/DrawImage` コマンド列を生成し、backend 非依存化。
  - **完了条件**: 同一コマンド列を native/tauri 互換で再生可能。

### Epic 4: Web Platform 機能（最低限の実利用ライン）
- [ ] **E4-T1 JavaScript 実行基盤の統合**
  - JS runtime を導入し、DOM 読み取り/イベント dispatch まで接続。
  - **完了条件**: 静的サイト + 軽量 SPA の初期描画が完了する。
- [ ] **E4-T2 イベントループ/タスクキュー実装**
  - microtask/macrotask の実行順序を定義し、タイマー API を追加。
  - **完了条件**: Promise/timeout を使うページでハングしない。
- [ ] **E4-T3 ストレージ/クッキー/権限モデル**
  - origin 単位で cookie/local storage を管理。
  - **完了条件**: ログイン状態を持つサイトで再訪時セッション維持。

### Epic 5: ネットワーク/セキュリティ
- [ ] **E5-T1 HTTP スタック拡張**
  - redirect policy、cache-control、content-encoding、timeout 制御を統合。
  - **完了条件**: 計測対象サイトで失敗率と表示時間を改善。
- [ ] **E5-T2 Same-Origin/CORS の導入**
  - Fetch/XHR ポリシー評価と preflight を実装。
  - **完了条件**: セキュリティテストで CORS 回避が不可。
- [ ] **E5-T3 TLS/証明書エラー UX**
  - 証明書期限切れ・自己署名の警告画面と例外登録。
  - **完了条件**: 危険接続時に必ず interstitial が表示される。

### Epic 6: UI/UX（タブ・アドレスバー・開発者体験）
- [ ] **E6-T1 タブストリップ改善**
  - ピン留め、ミュート、複製、ドラッグ並べ替え。
  - **完了条件**: 20 タブ以上で操作遅延が許容値内。
- [ ] **E6-T2 アドレスバー（Omnibox）**
  - URL/履歴/検索候補を統合し、キーボード操作を最適化。
  - **完了条件**: 主要ショートカットで遷移完了まで一貫動作。
- [ ] **E6-T3 ダウンロードマネージャ**
  - 進捗表示、一時停止/再開、保存先ポリシー。
  - **完了条件**: 大容量ファイルで再開可能ダウンロードが成功。

### Epic 7: 観測性・品質保証
- [ ] **E7-T1 ブラウザ KPI ダッシュボード**
  - 成功率、クラッシュ率、FCP 相当、メモリ使用量を定期収集。
  - **完了条件**: CI/ nightly で時系列比較が可能。
- [x] **E7-T2 Web 互換スモークスイート**
  - 代表サイト群のナビゲーション/描画/入力を自動検証。
  - **完了条件**: リグレッションを PR 時点で検知。
  - 進捗メモ: PR 軽量版 + nightly 拡張版の二段構成で導入し、失敗時ログを artifact 保存。
- [ ] **E7-T3 クラッシュレポート基盤**
  - panic/minidump 収集、シンボル管理、再現手順テンプレート化。
  - **完了条件**: 上位クラッシュ原因の特定時間を短縮。

### Epic 8: リリース/移行完遂（WebView-free デフォルト化）
- [ ] **E8-T1 `adapter_native` をデフォルト起動へ切替**
  - feature flag で段階ロールアウトし、失敗時に互換経路へフォールバック。
  - **完了条件**: 安定チャネルで WebView なし既定起動を達成。
- [ ] **E8-T2 `adapter_tauri` の互換範囲を最小化**
  - 旧 command 利用統計を取り、未使用 API を削減。
  - **完了条件**: 互換レイヤの保守コストを定量的に削減。
- [ ] **E8-T3 GA 判定ゲート運用**
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
1. [ ] **NF-T1 E3-T2 最小レイアウトエンジン v1 を実装**
   - block/inline の normal flow、margin/padding/border の計算を本実装へ。
   - 代表ページ群で「要素重なり崩れ率」を定量化し、基準値をドキュメント化。
2. [ ] **NF-T2 E3-T3 ペイントコマンド backend 非依存化**
   - `DrawRect/DrawText/DrawImage` を標準描画コマンドとして固定。
   - tauri/native 双方で同一コマンド列再生テストを CI に追加。
3. [ ] **NF-T3 描画回帰テスト拡張**
   - RenderTree と paint command の snapshot テストを導入し、差分を可視化。

### Sprint G（続く2週間）: Web Platform 最低ライン
4. [ ] **NG-T1 E4-T1 JavaScript 実行基盤を統合**
   - DOM 読み取り・イベント dispatch を最小接続し、軽量 SPA 初期描画を成立。
5. [ ] **NG-T2 E4-T2 イベントループ/タスクキュー実装**
   - microtask/macrotask の順序を仕様化し、Promise/timeout のハングを解消。
6. [ ] **NG-T3 E4-T3 ストレージ/クッキー基盤**
   - origin 単位の cookie/local storage 管理と永続化ポリシーを実装。

### Sprint H（並行）: ネットワーク/セキュリティ強化
7. [ ] **NH-T1 E5-T1 HTTP スタック拡張**
   - redirect policy / cache-control / content-encoding / timeout を統合。
8. [ ] **NH-T2 E5-T2 Same-Origin/CORS 実装**
   - Fetch/XHR の preflight 判定を含むポリシー評価を導入。
9. [ ] **NH-T3 E5-T3 TLS エラーUX**
   - 証明書エラー interstitial + 例外登録フローを追加。

### Sprint I（GA前）: デフォルト切替・運用完成
10. [ ] **NI-T1 E8-T1 native デフォルト起動の段階ロールアウト**
    - feature flag とフォールバックを運用し、安定チャネルへ展開。
11. [ ] **NI-T2 E8-T2 互換 command の利用統計/削減**
    - `adapter_tauri` の未使用 API を削減し、保守対象を最小化。
12. [ ] **NI-T3 E8-T3 GA 判定ゲート運用**
    - 性能・安定性・互換性しきい値を CI/release pipeline に強制適用。

### 完了基準（更新）
- [ ] WebView なし既定起動で、主要シナリオ（遷移/戻る進む/検索/タブ/軽量SPA）が連続成功する。
- [ ] KPI（成功率・クラッシュ率・平均表示時間）が GA しきい値を連続で満たす。
- [ ] `adapter_tauri` は互換層として最小維持され、コア機能追加は `adapter_native` のみで成立する。
