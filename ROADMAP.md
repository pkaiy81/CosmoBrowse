# CosmoBrowse Roadmap v2 (Tauri-first, UI-switchable)

## 0. 結論（v2 方針）
- 開発は **Tauri を継続採用**。
- ただし構成は **UI 交換可能**を前提にし、`adapter` を差し替えるだけで `egui/iced/winit` へ移行できる設計にする。
- Rust 資産（検索・ネットワーク・レンダリング）は `saba_core` を中心に再利用する。

---

## 1. 現状確認（2026-03 時点）
- Rust toolchain: `stable`（`saba/rust-toolchain.toml`）。
- Tauri (Rust): `tauri = "2"`, `tauri-build = "2"`, `tauri-plugin-opener = "2"`。
- Tauri (JS): `@tauri-apps/api = "^2"`, `@tauri-apps/cli = "^2"`。
- Frontend: `vite = "^6.0.3"`, `typescript = "~5.6.2"`。
- 現在の Tauri 側はテンプレート段階（`greet` command のみ）。

---

## 2. アーキテクチャ v2（UI 差し替え前提）

## 2.1 レイヤ
1. **Core (`saba_core`)**
   - Browser/Page/DOM/Parser/Renderer などのドメインロジック。
   - UI 依存禁止。
2. **Application (`saba_app` 新規)**
   - ユースケース API を提供（`open_url`, `navigate`, `search`, `tabs`）。
   - DTO（画面向けデータ）を定義し、Core の内部型を隠蔽。
3. **Adapter (`adapter_tauri`, 将来 `adapter_egui` 等)**
   - UI フレームワーク固有処理（command/event/state binding）。
   - `saba_app` を呼ぶだけに限定。

## 2.2 依存ルール
- `adapter_* -> saba_app -> saba_core` の単方向のみ許可。
- UI が `saba_core` を直接 import するのは禁止。
- Core の内部構造体をフロントへ直接返さない（必ず DTO に変換）。

## 2.3 DTO の最小セット（v2）
- `NavigationState { can_back, can_forward, current_url }`
- `TabSummary { id, title, url, is_active }`
- `RenderSnapshot { title, text_blocks, links, diagnostics }`
- `AppError { code, message, retryable }`

---

## 3. 実装ロードマップ v2（10週間）

## Phase A: 土台再編（Week 1-2）
- [ ] `saba_app` crate 新設。
- [ ] workspace に UI/Tauri crate を正式参加。
- [ ] `AppService`（同期/非同期 API）定義。
- [ ] DTO / Error 体系定義。

**DoD**
- `cargo check --workspace` 成功。
- Tauri が `saba_app` 経由のみでコンパイル可能。

## Phase B: 最小ブラウズ体験（Week 3-4）
- [x] `open_url`, `reload`, `back`, `forward` 実装。
- [x] URL バー + ナビゲーションボタン + 本文表示 UI。
- [x] エラー表示（ネットワーク/パース失敗）。

**DoD**
- 主要3サイト程度で遷移と戻る/進むが動作。

## Phase C: タブ・検索統合（Week 5-6）
- [x] タブ API（`new/switch/close/list`）。
- [x] Rust 検索機能を `saba_app::search(query)` として統合。
- [x] 検索結果から遷移までの一連フローを接続。

**DoD**
- 複数タブ + 検索遷移がクラッシュなく動作。

## Phase D: 描画品質・観測性（Week 7-8）
- [ ] `RenderSnapshot` を段階拡張（style/layoutメタ情報）。
- [ ] ログ/メトリクス（遷移時間、失敗率、主要エラーコード）。
- [ ] 代表ページのスモークテスト追加。

**DoD**
- 代表ページ群で表示成功率と失敗理由が取得可能。

## Phase E: UI 交換検証（Week 9-10）
- [ ] `adapter_cli` or `adapter_egui` の最小 PoC を追加。
- [ ] 同一 `saba_app` API で動作確認。

**DoD**
- Tauri 以外の adapter で `open_url/get_snapshot` が動く。

---

## 4. バージョン運用 v2（見直し結果）

## 4.1 Rust
- 現状 `stable` は妥当。
- **v2 推奨**:
  - 開発中: `stable` 追従で速度優先。
  - ベータ前: `channel = "1.xx.0"` に pin（再現性重視）。
  - 全 crate に `rust-version` を記載して下限を固定。

## 4.2 Tauri
- 現状の major=2 は妥当。
- **v2 推奨**:
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
1. [x] `saba_app` crate を追加。
2. [x] `AppService` trait + 実装を作成。
3. [x] `open_url` と `get_render_snapshot` を `saba_app` に実装。
4. [x] Tauri command は上記2つのみ先に接続。
5. [x] フロントに URL バー + 読み込みボタン + 表示領域を実装。
6. [x] エラーDTOを画面表示。
7. [x] CIに workspace check/test + frontend build を追加。

**2週間の完了基準**
- URL入力から本文表示まで一気通貫で動作。
- UI 直結ロジックが `saba_app` の外に漏れていない。

---

## 6. リスクと対策（v2）
- **Tauri command 肥大化**
  - 対策: command は引数検証 + `saba_app` 呼び出しのみ。
- **Core 型漏れでUI依存固定化**
  - 対策: DTO境界を厳守、レビュー項目化。
- **依存更新の破壊的影響**
  - 対策: 月次更新 + スモークテスト + freeze期間設定。

---

## 7. KPI（v2）
- ナビゲーション成功率
- 平均ページ表示完了時間
- コマンド失敗率（`open_url`, `search` など）
- クラッシュ率
- 別adapterで再利用できた API 比率
