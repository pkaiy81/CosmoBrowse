# 専用セッション指示書: Phase 2.3 「float / clear / BFC」

> 独立セッション用。着手前に `saba/HANDOFF.md` 全体と本書を読むこと。プランが **「古典レイアウト最難関」** と位置づける項目。**reftest 先行**で進める。

## 背景 / 現在地

- レイアウトは `cosmo_engine/src/renderer/layout/layout_view.rs`(`build_layout_tree` → `update_layout`)+ `layout_object.rs`(`compute_size` 上→下、`compute_position`)。
- **float / clear は未実装**。`float` プロパティはカスケードで無視され、要素は通常フロー(block/inline)で配置される。
- BFC(ブロック整形コンテキスト)の明示概念は無い。overflow/flex/grid item などの成立条件も未整理。
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
