# CosmoBrowse

CosmoBrowse は Rust + Tauri で実装している実験的ブラウザです。
現在は **HTTP / HTTPS の取得、文字コード判定（Shift_JIS 含む）、frameset ページの分割表示、ネイティブ scene レンダリング** までをサポートしています。

## HTTPS ページ表示の現状

- `reqwest` ベースのローダーで `https://` URL を取得します。
- リダイレクト追従、キャッシュ再検証（ETag）、TLS エラー分類を含む取得パイプラインを実装済みです。
- 取得後は HTML デコードと frameset 解析を行い、`scene_items` を使って UI 側で描画します。

詳細: [docs/https-display-implementation.md](docs/https-display-implementation.md)

## ローカル確認手順（HTTPS）

### 1. CLI でスナップショット確認

```bash
cd saba
cargo run -p adapter_cli --offline -- get-snapshot https://abehiroshi.la.coocan.jp/
```

確認ポイント:
- `root` 配下に `left` / `right` フレームが構築されること
- diagnostics にデコード・取得関連の情報が出ること

### 2. ターゲット付きリンク遷移確認

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

### 3. Tauri UI での目視確認

```bash
cd saba/ui/cosmo-browse-ui
npm install
npm run tauri dev
```

確認ポイント:
- URL 欄に `https://abehiroshi.la.coocan.jp/` を入力して表示できること
- 左メニュークリックで右フレームが切り替わること

## アーキテクチャ資料

- ブラウザ全体インデックス: [docs/cosmobrowse-browser-architecture.md](docs/cosmobrowse-browser-architecture.md)
- Runtime Topology: [docs/architecture/runtime-topology.md](docs/architecture/runtime-topology.md)
- Render Pipeline: [docs/architecture/render-pipeline.md](docs/architecture/render-pipeline.md)
- Integrated Sequence: [docs/architecture/integrated-sequence.md](docs/architecture/integrated-sequence.md)

