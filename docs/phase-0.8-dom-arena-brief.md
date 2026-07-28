# 専用セッション指示書: Phase 0.8 / 3.0 「DOM アリーナ移行」(設計判断 D1)

> 独立セッション用。着手前に `saba/HANDOFF.md` 全体・`docs/modernization-plan.md` の D1・本書を読むこと。**最リスクの純リファクタ**。ソース互換アクセサで呼び出し側変更を最小化する。

## 背景 / 現在地

- DOM は `cosmo_engine/src/renderer/dom/node.rs` の `Node`(`Rc<RefCell<Node>>`)。親/兄弟/last_child は `Weak`、first_child/next_sibling は `Rc`(saba 本のポインタモデル)。
- `Rc<RefCell<Node>>` は engine/runtime/script 全体で使用(型参照は ~11 ファイル、実アクセスはもっと多い)。
- `cosmo_script` は DOM を Boa GC の外に置き、`PageState.script_dom: Rc<RefCell<Node>>` として保持、`NodeHandle`(`#[unsafe_ignore_trace] Rc<RefCell<Node>>`)で Boa オブジェクトに紐付け。

## ゴール（D1）

DOM を **アリーナ + `NodeId(u32)`** へ: `Document { nodes: Vec<NodeSlot>, free: Vec<u32> }`。アクセスは既存 API 名を鏡写しにした `Document` メソッド経由(`first_child(id)` 等)にして呼び出し側変更を最小化。

## 動機（なぜやるか）

- **Boa 連携が根本的に安全化**: GC オブジェクトに `Copy` な `NodeId` を持たせられる(現状は `Rc` を捕獲不可のため thread_local + `NodeHandle` で回避している)。
- **インクリメンタルレイアウト(Phase 4.1)の dirty bit 置き場**。
- `Node PartialEq` が kind 比較という既存の罠を根治。
- 深い再帰 Drop(64MiB スタック)回避。

## 難所

1. **big-bang になりがち**。ソース互換の `Document` アクセサ API を先に整備し、`Rc<RefCell<Node>>` を返していた箇所を段階的に `NodeId` + `&Document` に置換。
2. HTML パーサ(`html/parser.rs`)がツリー構築で大量にポインタ操作 → アリーナ API へ移植。
3. `LayoutView::build_layout_tree` / `layout_object` が DOM を walk → `NodeId` ベースに。
4. `cosmo_script`: `PageState.script_dom`、`NodeHandle`、`node_key`(現 `Rc::as_ptr`)、wrapper cache のキー、全 DOM API を `NodeId` + `Document` 参照へ。`Document` をどこが所有し script からどう参照するか(thread_local/PageState 経由)を設計。
5. `api.rs`(get_element_by_id/query_selector 等)と `dom_node_selected`(セレクタマッチャ)も `NodeId` 走査に。

## 推奨アプローチ（段階的・各段でテスト green）

1. `Document`(アリーナ)+ `NodeId` 型と、既存名を鏡写しにしたアクセサ(first_child/next_sibling/parent/kind/attributes…)を追加。**旧 `Rc<RefCell<Node>>` と併存させない**(型を切替える)。
2. HTML パーサをアリーナ構築へ。html5lib 対応数を維持(`cosmo_engine/tests/html5lib.rs`、BASELINE_PASSES は raise-only)。
3. レイアウト(build_layout_tree/layout_object/api/selector)を `NodeId` 走査へ。
4. `cosmo_script`: `NodeHandle { id: NodeId }` に変え、`Document` を PageState が所有(または runtime が所有し PageState が参照)。wrapper cache キーを NodeId に。全 DOM API 移植。thread_local の DOM は `PageState` の Document に。
5. 全クレートのコンパイル・全テスト green を各段で確認。

## 検証

- `cd saba && cargo test`(全クレート)、`python3 scripts/run_layout_reftests.py`(12/12、画素一致)。
- html5lib 通過数が減らないこと。cosmo_script の DOM テスト全 green。
- 挙動変更ゼロ(純リファクタ)。フィクスチャ画素一致。

## 撤退ライン

big-bang が破綻したら、**Boa 連携が要求する最小範囲**(script が触るサブツリーだけ NodeId、他は Rc 併存)に絞るのは避ける(型併存は複雑化)。むしろ段階を細かく切り、各段でコンパイルを通す。どうしても無理なら着手を見送り、現行 thread_local + NodeHandle 方式を継続(Phase 4.1 の dirty bit は Node 上の `Cell` で代替可能)。

## 関連ファイル

- `saba/cosmo_engine/src/renderer/dom/node.rs`(enum は :240 付近)、`dom/api.rs`。
- `saba/cosmo_engine/src/renderer/html/parser.rs`(ツリー構築)。
- `saba/cosmo_engine/src/renderer/layout/layout_view.rs` / `layout_object.rs`、`style/selector.rs`。
- `saba/cosmo_script/src/lib.rs`(`PageState`/`NodeHandle`/`node_key`/DOM API)。
