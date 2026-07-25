# 専用セッション指示書: Phase 2.5 「インライン行ボックス本実装」

> 独立セッション用。着手前に `saba/HANDOFF.md` 全体と本書を読むこと。プランが **「回帰面積最大」** と位置づける項目。**1マイルストーンだけ新旧を LayoutView フラグで A/B し、reftest 比較後に旧経路削除**。

> **⚠ 2.3 と結合(2026-07-25 知見)**: float の回り込み(行の利用可能幅短縮)は本実装の行ボックスが前提。**2.3 floats と一体で実施**すること。

## 背景 / 現在地

- 現行のインラインは近似実装: `layout_object.rs::compute_size` の Text 経路で `split_text(text, font, bold, max_width)`(`layout/text` 系)により幅で折り返し、per-char アドバンステーブル(DejaVu 較正)で計測。行ボックスの明示概念は弱く、inline 要素の内部折り返し・ベースライン整合はパッチ的(`align_inline_baselines`)。
- 既知の残(HANDOFF §): 内部で折り返す inline **要素**が bbox のまま、行内の複雑な整合、UAX#14 非対応(任意位置で折り返しうる)。
- 依存: Phase 2.3(float)が「行の利用可能幅を狭める」ため、真のライン単位回り込みは本実装が要る。

## ゴール

本物の **インライン整形コンテキスト(IFC)**: 行ボックス(line box)を構築し、inline / inline-block / text / 置換要素を行に詰め、`FontMetricsProvider` の ascent/descent でベースライン整合。**UAX#14 行分割**(`unicode-linebreak` クレート)+ **書記素**(`unicode-segmentation`)+ overflow-wrap/word-break 基本 + ellipsis。float の帯で各行の利用可能幅を狭める。

## 難所

1. **回帰面積が最大**。既存の per-char split_text に依存した多くのフィクスチャ挙動が変わる。→ **A/B フラグ必須**(`LayoutView` に旧/新経路スイッチ)、reftest で新旧差分を精査してから旧削除。
2. per-char 計測と split の丸め会計は「計測と分割で同一の per-char 丸め」でないと自己折り返しでズレる(HANDOFF の既知の罠)。新経路でも計測一貫性を厳守。
3. inline-block / 置換要素(img)の行内配置、ベースライン、行の高さ算出(line-height、font metrics)。
4. float との結合(2.3): 行ボックス生成時に現在 Y の float エッジで開始 X/幅を決める。

## 推奨アプローチ（段階的）

1. **LineBox / IFC 骨格**: BFC/ブロック内で連続する inline 群を行ボックス列に整形するモジュール(`layout/inline.rs` 等)。まず text のみ。
2. **UAX #14 行分割**: `unicode-linebreak` を導入(データクレートは方針上 OK)。分割可能位置を求め、幅で行に詰める。書記素は `unicode-segmentation`。
3. **メトリクス**: `FontMetricsProvider` の ascent/descent で行の高さ・ベースライン。inline-block / img の参加。
4. **A/B**: `LayoutView` に `COSMO_NEW_INLINE`(env)等でスイッチ。reftest を新旧で撮り比較。
5. **float 連携**(2.3 と合流): 行の利用可能幅を float 帯で短縮。
6. 差分検証後に**旧インライン経路を削除**、フラグも撤去。

## 検証

- `cd saba && cargo test`、`python3 scripts/run_layout_reftests.py`。新規 inline reftest 群。
- **A/B**: 新経路 ON/OFF で全 reftest + フィクスチャ(HN/abehiroshi/Wikipedia/MDN)を撮り、差分を1件ずつ承認。承認後に旧削除・再ベースライン(専用コミット)。
- npr/長文記事の折り返し・overflow-wrap を確認。

## 撤退ライン

UAX#14 まで届かない場合、**行ボックス骨格 + 既存 split_text の計測を新構造に載せ替え**までを landing(A/B で full 一致を担保)。UAX#14/word-break は追補。旧経路削除は差分ゼロを確認できてから。

## 関連ファイル

- `saba/cosmo_engine/src/renderer/layout/layout_object.rs` — Text/inline の `compute_size`、`split_text` 呼び出し、`align_inline_baselines`。
- `saba/cosmo_engine/src/renderer/layout/` の text/measure 系。
- `saba/cosmo_engine/src/renderer/layout/layout_view.rs` — 経路スイッチ、`align_inline_baselines`。
- `saba/Cargo.toml` — `unicode-linebreak` / `unicode-segmentation` 追加。
