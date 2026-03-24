# CosmoBrowse

CosmoBrowse は Rust + Tauri で実装している実験的ブラウザです。
現在は **HTTP / HTTPS の取得、文字コード判定（Shift_JIS 含む）、frameset ページの分割表示、ネイティブ scene レンダリング** までをサポートしています。

## 実装スタック（現状と目標）

- ネットワーク取得、HTML/文字コード処理、frameset 解析、リンクターゲット遷移などのブラウザロジックは Rust 実装です。
- **現状**のデスクトップ UI は Tauri を利用しており、表示ランタイムとして OS の WebView を使います（Windows: WebView2 / macOS: WKWebView / Linux: WebKitGTK）。
- **目標**は、表示レイヤも Rust ネイティブ化し、WebView 依存をなくすことです。

## WebView 非依存化の設計方針

1. `adapter_* -> cosmo_runtime -> cosmo_core` の依存方向を維持したまま、`adapter_tauri` を `adapter_native` へ段階置換する。
2. UI は Web 技術（HTML/CSS）ではなく、Rust 側で `scene_items` を直接描画する（winit/pixels/tao 等のネイティブウィンドウ基盤を想定）。
3. IPC 契約 (`dispatch_ipc`) は維持し、既存機能を壊さずに移行できるよう互換レイヤを残す。
4. 移行完了後は Tauri/WebView 依存をオプション化し、標準実行経路をネイティブ描画に切り替える。

詳細は `docs/ROADMAP.md` と `docs/architecture/runtime-topology.md` を参照してください。

## 現時点の完成判定ステータス（ローカル実行前の確認用）

GA 判定は nightly artifact の `history/<history_key>/` 配下にある成果物を基準に確認します。
ローカル実行前に、次の「既知ギャップ」を先に把握してください。

- **連続達成の確認が未実施だと完成判定できない**
  - 確認キー: `ga-gate-report.json.consecutive_pass_streak` / `required_consecutive_passes`
  - 判定条件: `consecutive_pass_streak >= 3` かつ `release_blocked == false`
- **GA gate の pass 結果が未取得だと完成判定できない**
  - 確認キー: `ga-gate-report.json.gate_passed` と `checks[*].passed`
  - 判定条件: `gate_passed == true` かつ mandatory gate（success/crash/display/layout）がすべて `true`
- **重大クラッシュ情報がないと安全判定ができない**
  - 確認キー: `kpi_summary.json.failure_classification.crash` と `latest_crash_report.json`
  - 判定条件: 原則 `crash.count == 0`。`count == 1` の場合は crash report に再現/切り分け情報（`transport` / `active_url` / `last_command` / `build_id` / `commit_hash`）が揃っていること

ローカルで smoke を実行して成果物を作るまでは、上記 3 点は **未判定（known gap）** として扱ってください。


## 進捗確認（2026-03-24）

`docs/ROADMAP.md` のチェックリストを基準に確認した結果、**未完了は Epic 6 の E6-T3（ダウンロードマネージャ）と GA 判定の連続達成のみ**です。

- 完了済み: E1〜E5, E7〜E8, および E6-T1/E6-T2 は完了。
- 未完了: E6-T3（大容量回帰スモーク運用）。
- 2026-03-24 追記: HTTP Range resume の厳密検証（`Content-Range` オフセット一致 + `ETag`/`Last-Modified` 再検証不一致時の full restart）と、保存先ポリシー UI + session restore 後の保持を実装済み。
- 未判定: GA gate の `consecutive_pass_streak >= 3` を満たす連続 nightlies。

### 未完了に対する新規タスク（2026-03-24 作成）

1. **DL-T1: HTTP Range resume strict validation**
   - `Accept-Ranges` / `Content-Range` / `ETag` / `Last-Modified` の整合チェックを実装し、resume 可否を deterministic に判定する。
2. **DL-T2: 保存先ポリシー UI と設定永続化**
   - 「毎回確認 / 既定フォルダ / サイト別ポリシー」を選べる設定導線を UI に追加し、session restore 後も保持する。
3. **DL-T3: 大容量ダウンロード回帰スモークの定常化**
   - pause/resume/retry を含む 1GB 級の検証ケースを nightly に追加し、完了ファイルの checksum 検証まで自動化する。

### 実装完了後に行うローカル確認・配布物生成

未完了タスクを解消した後は、次の手順で完成確認と配布物生成を実施してください。

- ローカル smoke: `python3 scripts/run_smoke_regression.py --mode nightly --artifacts-dir smoke-artifacts/nightly`
- GA gate 判定: `python3 scripts/evaluate_release_gate.py --history-root smoke-artifacts/nightly/history --output smoke-artifacts/nightly/ga-gate-report.json`
- 連続達成確認: `python3 scripts/check_release_streak.py --history-root smoke-artifacts/nightly/history --report smoke-artifacts/nightly/ga-gate-report.json`
- Windows portable 配布物作成（Rust/Node が無い実行環境向け）: `pwsh -File scripts/build-cosmobrowse-portable.ps1 -Version <version> -OutDir <out_dir>`

配布物の利用者は zip を展開して `cosmo-browse-ui.exe` を起動するだけで試せます（実行端末に Rust/Node は不要）。

## HTTPS ページ表示の現状

- `reqwest` ベースのローダーで `https://` URL を取得します。
- リダイレクト追従、キャッシュ再検証（ETag）、TLS エラー分類を含む取得パイプラインを実装済みです。
- 取得後は HTML デコードと frameset 解析を行い、`scene_items` を使って UI 側で描画します。

詳細: [docs/https-display-implementation.md](docs/https-display-implementation.md)

## ローカル起動方法

### A. 開発環境で起動する（Rust/Node あり）

#### 1. CLI でスナップショット確認

```bash
cd saba
cargo run -p adapter_cli --offline -- get-snapshot https://abehiroshi.la.coocan.jp/
```

確認ポイント:
- `root` 配下に `left` / `right` フレームが構築されること
- diagnostics にデコード・取得関連の情報が出ること

#### 2. ターゲット付きリンク遷移確認

```bash
cd saba
cargo run -p adapter_cli --offline -- activate-link \
  https://abehiroshi.la.coocan.jp/ \
  root/left \
  https://abehiroshi.la.coocan.jp/movie/eiga.htm \
  right
```

確認ポイント:
- `target="right"` 相当で右フレームのみ更新されること

#### 3. 現行 UI (Tauri) での目視確認

```bash
cd saba/ui/cosmo-browse-ui
npm install
npm run tauri dev
```

確認ポイント:
- URL 欄に `https://abehiroshi.la.coocan.jp/` を入力して表示できること
- 左メニュークリックで右フレームが切り替わること

### B. Rust/Node を入れずに起動する（現行の配布済みバイナリ）

エンドユーザー環境での実行は、ソースからビルドせずに **配布済み zip / 実行ファイル**を使用します。

1. 配布された `cosmo-browse-ui-<version>-windows-portable.zip` を展開する。
2. 展開先の `cosmo-browse-ui.exe` を実行する。
3. URL 欄に `https://abehiroshi.la.coocan.jp/` を入力して動作確認する。

配布物の作り方は `docs/windows-portable-distribution.md` を参照してください。
実行側の PC には Rust / Node は不要です（現行配布物では Windows の WebView2 ランタイム＝Edge WebView が必要）。

### C. WebView なしで Core/Adapter を確認する（native_ipc_cli）

`adapter_native` に含まれる `native_ipc_cli` は、Tauri/WebView を起動せずに
`dispatch_ipc` 契約を直接叩ける確認用 CLI です。

```bash
cd saba
cargo run -p adapter_native --bin native_ipc_cli -- once '{"type":"open_url","payload":{"url":"https://abehiroshi.la.coocan.jp/"}}'
```

JSON をファイルで管理したい場合は `@<path>` 指定も使えます。

```bash
cd saba
cargo run -p adapter_native --bin native_ipc_cli -- once @./requests/open-url.json
```

標準入力モードで複数リクエストを順次流すこともできます。

```bash
cd saba
echo '{"type":"open_url","payload":{"url":"https://abehiroshi.la.coocan.jp/"}}' | cargo run -p adapter_native --bin native_ipc_cli -- stdin
```

## リグレッションスモーク（navigation / rendering / input）

`scripts/run_smoke_regression.py` はローカル fixture サーバーを起動し、
`native_ipc_cli` に対して代表ケースの検証を行います。

- 代表ケース: 静的ページ / 軽量 SPA / redirect / エラーページ
- 検証: `open_url` -> snapshot 取得 -> タイトル・主要テキスト・リンク数
- 追加シナリオ: `back` / `forward` とタブ切替の最小フロー
- 失敗時: `AppMetricsSnapshot` とセッションログを `smoke-artifacts/` に保存

PR 向け軽量版:

```bash
python3 scripts/run_smoke_regression.py --mode pr --artifacts-dir smoke-artifacts/pr
```

nightly 想定のフル版:

```bash
python3 scripts/run_smoke_regression.py --mode nightly --artifacts-dir smoke-artifacts/nightly
```

## アーキテクチャ資料

- ブラウザ全体インデックス: [docs/cosmobrowse-browser-architecture.md](docs/cosmobrowse-browser-architecture.md)
- Runtime Topology: [docs/architecture/runtime-topology.md](docs/architecture/runtime-topology.md)
- Render Pipeline: [docs/architecture/render-pipeline.md](docs/architecture/render-pipeline.md)
- Integrated Sequence: [docs/architecture/integrated-sequence.md](docs/architecture/integrated-sequence.md)
