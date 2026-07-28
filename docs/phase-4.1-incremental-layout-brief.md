# 専用セッション指示書: Phase 4.1 「真のサブツリー部分再計算」

> このドキュメントは、Phase 4.1 の残りの大きな塊 **「変更サブツリーのみを再レイアウトする真のインクリメンタルレイアウト」** を、独立した集中セッションで実施するための指示書です。着手モデルはまず `saba/HANDOFF.md` 全体と本書を読んでください。

## 背景 / 現在地（2026-07-25 時点）

CosmoBrowse は Boa ベースの JS 実行(`cosmo_script`)+ プログレッシブ描画(`renderer_native/app_bridge.rs::AppBridge` が per-frame `LivePage` を保持)まで完成済み。レイアウトの再計算に関して、**安全な範囲の最適化は完了**している:

- **CSSOM キャッシュ** (`cosmo_runtime/src/layout/mod.rs`): `LivePage` が `resolve_cssom` で1回パースし、`layout_dom_with_style`/`layout_scene_only` で再利用。
- **render-tree snapshot スキップ** (`layout_scene_only`): LivePage は paint scene のみ生成。
- **DOM 変異世代カウンタ** (`cosmo_script::ScriptHost::dom_generation`): 変異が無ければ pump で再レイアウトをスキップ。
- **安全網 `COSMO_LAYOUT_ASSERT=1`**: `LivePage::pump_and_relayout` が full レイアウトも計算し scene の byte 一致を assert（`assert_matches_full`）。

**まだ未実装＝本セッションの対象**: 「DOM/スタイルが変わったとき、影響サブツリーのみを再計算し、変わっていない部分のレイアウト結果を再利用する」こと。現状は変化があると `LayoutView::new_with_viewport`（`cosmo_engine`）で**毎回ツリーを全再構築＋全カスケード＋全 update_layout**している。

## ゴール

`build_layout_tree` からの全再構築を、**dirty サブツリーのみの再構築 / 再カスケード / 再レイアウト**に置き換え、`COSMO_LAYOUT_ASSERT=1` 下で **常に full と byte 一致**を保ちながら、典型的な単一要素変更のレイアウトコストを下げる。

## 難所（必ず織り込むこと）

1. **ブロックフローは相互依存**: 子の高さ→親の高さ→後続兄弟の位置…と伝播する。局所変更でも影響は「最近傍のブロック整形コンテキスト(BFC)祖先」まで広がりうる。プラン許容どおり **無効化は BFC 祖先単位の粗さで可**。
2. **本番(assert OFF)では部分再計算のバグが silent mis-render になる**。最もピクセル一致を守っている layout 全体に影響するため、正しさは assert だけに依存せず設計で担保する。
3. `LayoutView::update_layout` には「一度だけ計算」ガード（例: table column hints、line-box 計測）がある。既存ツリーで再実行すると stale を再利用しうる。再計算対象の状態を明示的にリセットするか、サブツリー再構築で作り直すこと。
4. `@media` / `vw/vh/vmin/vmax` はビューポート依存。リサイズ時のツリー再利用は、これらが CSSOM に無い場合のみ安全（`StyleSheet` から検出可能）。

## 推奨アプローチ（段階的・各段で reftest + assert を green に保つ）

### Step A: DOM→LayoutObject 対応 + per-node layout-dirty
- `cosmo_engine` の `Node` に安定 id か、`LayoutObject` に生成元 `Rc<RefCell<Node>>` の弱参照を持たせ、**DOM ノード↔レイアウトオブジェクトの対応表**を作る。
- `cosmo_script` は変異チョークポイント（`set_attr_of` / tree ops / text setter / innerHTML / dataset）で**変異ノードを dirty マーク**（現状の `bump_dom_generation` と同じ箇所）。`ScriptHost` に `take_dirty_nodes()` を追加し、変異ノード集合を runtime へ渡す。

### Step B: 無効化スコープの算出
- dirty ノード集合から、**再レイアウトすべき最小の BFC 祖先**（`overflow≠visible`/flex/grid/root など、`cosmo_engine` の BFC 判定を再利用）を求める。複数 dirty があれば各スコープの和集合、または最近共通祖先。

### Step C: サブツリー再構築 + 部分 update_layout
- `LivePage` が `LayoutView` を永続保持。
- 変異時、対象 BFC サブツリーだけ `build_layout_tree`+カスケードで作り直し、既存ツリーに差し替え。
- `update_layout` を「サブツリールートから再計算し、そのサイズ差を祖先へ伝播（親の高さ・後続兄弟位置の再計算）」する形に拡張。まずは **BFC ルートから下＋その後の兄弟/祖先高さ再計算** の粗い版で可。
- **各段で `COSMO_LAYOUT_ASSERT=1` の全 reftest / 既存テストが green（＝ full と byte 一致）** を確認。乖離したらスコープを広げて full 側に寄せる。

### Step D: リサイズ再利用（任意・余力があれば）
- CSSOM に `@media`/viewport 単位が無い場合のみ、リサイズで**カスケード済みツリーを再利用**して `update_layout` だけ再実行。

## 検証（必須）

- `cd saba && cargo test`（全クレート）、`python3 scripts/run_layout_reftests.py`（12/12）。
- **`COSMO_LAYOUT_ASSERT=1` を付けて** reftest / runtime テストを回し、部分再計算が full と乖離しないことを常時担保。乖離は panic するので CI で検出できる。
- ベンチ: 大きめの CSS/DOM のページ（Wikipedia フィクスチャ）で pump のレイアウト時間を before/after 計測し、実利を示す。

## 撤退ライン

局所性が取れず「結局ほぼ full 再計算」になる、または assert 乖離が潰し切れない場合は、**Step A/B（dirty 追跡の土台）までを landing し、Step C は文書化した近似（BFC 祖先からの再計算のみ）で確定**してよい（プランの「工数超過時は近似にフォールバック可」に準拠）。

## 関連ファイル

- `saba/cosmo_runtime/src/layout/mod.rs` — `LivePage` / `layout_dom` 系 / `assert_matches_full`。
- `saba/cosmo_engine/src/renderer/layout/layout_view.rs` — `build_layout_tree` / `update_layout` / `LayoutView`。
- `saba/cosmo_engine/src/renderer/layout/layout_object.rs` — `LayoutObject` / `compute_size` / `compute_position` / BFC 判定。
- `saba/cosmo_script/src/lib.rs` — 変異チョークポイント / `dom_generation` / `PageState`。
