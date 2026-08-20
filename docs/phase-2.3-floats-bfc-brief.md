# 専用セッション指示書: Phase 2.3 「float / clear / BFC」

> 独立セッション用。着手前に `saba/HANDOFF.md` 全体と本書を読むこと。プランが **「古典レイアウト最難関」** と位置づける項目。**reftest 先行**で進める。

> **🟡 Phase 2.3 一部完了 (2026-08-20, PR #92)** — 不動点反復・float 配置・`clear`・shrink-to-fit まで
> landing(前提のレイアウトパス冪等化は #91)。実装で見つかった実バグ3件も修正済み
> (auto 幅 float がコンテナ全幅を取る / BFC が**継承した** float まで内包しようとして伸びる /
> 配置が自分の前回出力を入力にして下方ドリフト)。
>
> **⚠ 行短縮の到達範囲(実測値)**: reftest では確実に効く(float の右端 x=110 から行が始まる)。
> Wikipedia では **552 ブロックが float context を継承し、うち 264 で実際に行が短縮される** —
> つまり広く効いているが**完全ではない**。特定の float の箱(x 580..1000, y 975..1183)には
> 記事テキストの断片が 20 個食い込んだままで、キャプションと重なって見える。
>
> 原因は**継承の失敗ではない**(継承は届いている)。**BFC 境界でもない**(境界を無視させても
> 20→17 とほぼ不変)。残る容疑は `FloatContext::content_width` が**祖先の幅**のままで、
> `band()` の右端クランプが子孫の狭い content width と食い違う点
> (継承したが narrows=false になるブロックが 288 ある。その多くはこの食い違いで説明できる)。
> **次はここから測る。**
>
> **他の残**: `max_content_width` が粗く shrink-to-fit の精度を制限している。

> **✅ その前提だった「レイアウトパスの冪等性」は解消済み(2026-08-17)** — 反復設計はパスを
> 繰り返し実行して収束させるが、**パス自体が冪等でなかった**(実測: float 処理を全て無効にしても
> Wikipedia が 1パス→2パスで 80,017px 変化し収束しない)。原因は**テーブルセルの auto 幅**が
> `column_width_from_sibling_rows` 経由で**前パスの確定サイズ**を読み、出力が次パスの入力に
> なっていたこと(1回目は未確定で `None` → 一般分配、2回目は値が入り別の答え)。
> `update_layout` が先頭で幾何情報をリセットし、パスを「木・CSSOM・ビューポート」の純関数に修正。
> 冪等性テスト5本(**実 Wikipedia 文書 395KB を含む**)で固定済み。
>
> **反復を実装する際の注意**: 持ち越す状態は float の配置(`float_context`)**だけ**にし、
> リセット対象から除外すること。それ以外は毎パス clean から始める —
> 「(木, CSSOM, viewport, float_context) の純関数」が不動点反復に必要な形。
>
> なお 2026-08-02 のブランチ `wip/block-level-floats-2026-08-02` に反復の実装一式が
> 「NOT FOR MERGE」で保存してある(reftest の5ケースは全て green)。冪等化後の base に
> 載せ替えるところから再開できる。そこで見つかった実バグ2件(containment の暴走、float の
> 下方ドリフト)の修正も含まれる。

## 状況(2026-07-29 更新)

**インライン文脈内の float は landing 済み**(`4b6f4a6` 土台 / `7966dfd` 結線 / `16a9497`)。
`FloatContext`(`place`/`band`/`clearance`/`lowest_bottom`/`translated`、11テスト)、
`establishes_block_formatting_context`(CSS2.2 §9.4.1)、reftest `floats`(5ケース)。
IFC が各行の使用可能幅を context に問うので、**含みブロックの中身が全てインラインなら回り込みが動く**
(`<div><img style="float:left">text…</div>`)。CSS2.2 §9.5 の「脇に入らない行は下へ送る」も実装済み。

**残るのはブロックレベルの回り込みのみ**: 非 float の兄弟に**ブロック**があると context が成立せず、
float は通常フローに落ちる。reftest `floats` の該当ケースが「NOT YET」として固定している。

### 一度試して revert した実装と、その理由(重要)

「通常フローの積み上げなら各子の Y はサイズパスで計算できる(先行する子の outer height の総和)」
として実装 → **margin collapsing 等 `compute_position` の仕事を推定しきれず**、さらにその結果を
`inline_offset` で子に強制したため**位置パスを上書き**した。reftest は正しく見えたが
**Wikipedia の段落同士が重なった**ため revert(`16a9497` のコミットメッセージに詳細)。
**教訓: サイズパスの時点で「この子が最終的にどこに来るか」は原理的に分からない。**

### 推奨する設計: サイズ↔位置の不動点反復

float の位置は**実位置**からしか決まらず、実位置は行の短縮に依存する — この循環を反復で解く:

```
iteration 1: 従来どおり(float も通常フロー)→ 位置確定
  ↓ 各 BFC ルートについて、iteration 1 の実位置で float を配置し
    FloatContext を BFC ルートに保存(`LayoutObject::float_context`)
iteration 2: サイズパスで各ボックスが「自分を含む BFC の context」を
    **前回の自分の Y** で translate して参照し、行を短縮 → 位置再計算
  ↓ float 集合が変わらなければ終了(通常 2 回で収束)
```

要点:
- **float にだけ** `inline_offset`(位置固定)を与える。**在フローの兄弟には与えない** — これが前回の失敗要因。
- `calculate_node_position` の out-of-flow 判定(現在 zero-size と absolute/fixed)に **float を追加**し、
  float が次兄弟のフローアンカーにならないようにする(CSS2.2 §9.5: float は流れから外れる)。
- 収束しない場合に備えて反復回数に上限を置く。

### 撤退ライン

反復が収束しない/回帰が大きい場合は、**float を含む BFC に限って**反復し、それ以外は現状維持。
それも難しければ現状(インライン文脈のみ)で確定し、reftest の NOT YET ケースを維持する。

## 旧・背景メモ

- レイアウトは `cosmo_engine/src/renderer/layout/layout_view.rs`(`build_layout_tree` → `update_layout`)+ `layout_object.rs`(`compute_size` 上→下、`compute_position`)。
- 影響例: Wikipedia のサムネイル(`float:right` + caption)、記事の回り込み。

## ゴール

`float:left/right` の要素をラインボックスの脇に配置し、後続のインライン/ブロックが回り込む。`clear:left/right/both` で回り込み解除。float は BFC 内でのみ相互作用し、BFC 成立条件(root / `overflow≠visible` / flex・grid item など)を実装。**reftest(golden PNG)を各機能とともに着地**。

## 難所

1. float は「行ボックスの利用可能幅を短くする」= **インラインレイアウトと密結合**。Phase 2.5(インライン行ボックス本実装)と相互依存。float 先行なら現行インライン経路に float 帯を差し込む近似から。
2. float の配置は「現在の Y における左右の float エッジ」を追跡する状態(float リスト)が要る。BFC ごとに管理。
3. 親の高さに float を含めるかは BFC の話(overflow:hidden な親は float を内包)。clearance の計算。
4. **回帰面積が広い**。float を入れると既存の通常フロー配置が動く箇所がある。reftest で必ず差分を可視化。

## 推奨アプローチ（段階的・各段で reftest green）

1. **BFC 判定 + float リスト構造**: `LayoutObject` に「この要素は BFC を確立するか」を判定するメソッド、BFC ルートに `Vec<FloatBox { side, rect }>` を持たせる。
2. **float 配置**: `compute_position` で float 要素を通常フローから外し、現在 Y の利用可能左右エッジに詰める。float どうしの積み上げ。
3. **回り込み**: 同 BFC 内の後続ブロック/ラインボックスの利用可能幅・開始 X を float 帯で狭める(まずはブロック単位の粗い回り込み → 2.5 でライン単位に精緻化)。
4. **clear**: `clear` 指定要素は、対応する側の float の下端まで Y を送る。
5. **親高さ/内包**: `overflow≠visible` な親は float を内包して高さに算入。
6. 各段で `float` reftest(~15、プラン受け入れ基準)+ WPT float 抜粋を追加。

## 検証

- `cd saba && cargo test`、`python3 scripts/run_layout_reftests.py`。
- 新規 float reftest ~15。Wikipedia フィクスチャのサムネイル回り込みが正常化。
- 既存 reftest 12/12 と HN/abehiroshi 画素一致を維持(float 導入で通常フローが動かないこと)。

## 撤退ライン

ライン単位の精密回り込みが 2.5 未実装で困難なら、**ブロック単位の粗い回り込み + clear + BFC 内包**までを landing し、ライン短縮は 2.5 と合流時に精緻化(文書化した近似で確定)。

## 関連ファイル

- `saba/cosmo_engine/src/renderer/layout/layout_view.rs` — `update_layout` / `calculate_node_position`。
- `saba/cosmo_engine/src/renderer/layout/layout_object.rs` — `compute_size` / `compute_position` / kind 判定。
- `saba/cosmo_engine/src/renderer/style/cascade.rs` — `float`/`clear` プロパティ解析(未対応なら追加)。
- `saba/testdata/reftests/` — golden 追加先。
