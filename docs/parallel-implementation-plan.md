# Parallel Implementation Plan

## 目的
複数トラックで並行開発する際の担当境界・依存関係・禁止領域を明確化し、衝突と手戻りを最小化する。

## トラック定義（6トラック）

### Track 1: Foundation
- **担当境界**
  - 共通基盤（共通ユーティリティ、エラー型、設定、ビルド/CI基盤）
  - ドキュメント整備（規約、開発フロー、テンプレート）
- **依存関係**
  - なし（最上流）
- **禁止領域**
  - レイアウト/レンダリングロジックの機能追加
  - JavaScript実行系の仕様変更

### Track 2: Layout
- **担当境界**
  - DOM/CSSOM からレイアウトツリー構築まで
  - ボックスモデル、サイズ/位置計算
- **依存関係**
  - Foundation 完了後に着手
- **禁止領域**
  - ペイント/描画パイプラインの仕様変更
  - JS実行・セキュリティ関連の変更

### Track 3: Render
- **担当境界**
  - DisplayItem 生成、描画順、ペイント処理
  - レンダリング結果の検証系
- **依存関係**
  - Foundation 完了後
  - Layout のデータ構造変更を取り込んで実装
- **禁止領域**
  - レイアウト計算アルゴリズムの直接変更
  - JSエンジン内部仕様の変更

### Track 4: JavaScript
- **担当境界**
  - パーサ/AST/ランタイム、DOM API ブリッジ
  - JS実行フローとイベント連携
- **依存関係**
  - Foundation 完了後
  - Layout/Render の公開インターフェースを利用
- **禁止領域**
  - ネットワーク/セキュリティ境界ポリシーの単独変更

### Track 5: Security
- **担当境界**
  - URL/オリジン制約、外部リソース取得制御
  - 入力検証、危険操作のガード、監査ログ方針
- **依存関係**
  - Foundation 完了後
  - JavaScript 実行経路と接続して最終適用
- **禁止領域**
  - レンダリング出力仕様の変更
  - DevTools のUI/UX変更

### Track 6: Process/DevTools
- **担当境界**
  - 開発プロセス整備、デバッグ導線、運用ドキュメント
  - DevTools/診断補助機能
- **依存関係**
  - Foundation 完了後
  - Layout/Render/JavaScript/Security の主要変更を取り込んで仕上げ
- **禁止領域**
  - 各実行系コアロジックの仕様変更

## ブランチ命名規約
- 統一フォーマット: `feature/<track>-<scope>`
- `<track>` は以下のいずれか: `foundation`, `layout`, `render`, `js`, `security`, `process`
- `<scope>` は作業範囲をケバブケースで表す

### 命名例
- `feature/foundation-parallel-dev`
- `feature/layout-inline-formatting`
- `feature/render-display-list`
- `feature/js-dom-events`
- `feature/security-origin-policy`
- `feature/process-devtools-logging`

## マージ順（固定）
1. foundation
2. layout / render
3. js / security
4. process / devtools

> 同一レイヤー（`layout` と `render`、`js` と `security`）は相互レビュー後にマージする。

## 全後続タスク共通チェックリスト
- [ ] 作業開始時に新規ブランチを `feature/<track>-<scope>` で作成した
- [ ] 担当トラックの禁止領域を侵していない
- [ ] 上流依存（マージ順）を確認した
- [ ] PRテンプレートの必須項目をすべて入力した
