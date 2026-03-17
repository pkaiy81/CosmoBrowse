# Layout regression policy

`cosmo_core` の normal flow（block/inline）向け回帰基準。

## Representative page set

CI smoke (`scripts/run_smoke_regression.py`) は以下を代表ケースとして利用する。

- `static_page` (`/static`): 通常テキスト + 複数リンク
- `redirect` (`/redirect`): リダイレクト後の通常文書
- （nightly のみ）`lightweight_spa`, `error_page`

## 崩れ判定ルール

`root_frame.render_tree` を使い、各ノードで次を判定する。

1. **欠落（missing）**: `#text` 以外で `width <= 0` または `height <= 0`。
2. **はみ出し（overflow_box_model）**: `content_width > width` または `content_height > height`。
3. **重なり/逸脱（child_out_of_parent）**: 子の border-box が親 border-box の外へ出る。

## 崩れ率

- `breakage_rate = layout_failures_case_count / evaluated_case_count`
- CI 許容閾値: **10% 以下**（`LAYOUT_BREAKAGE_THRESHOLD = 0.10`）
- 集計結果: `smoke-artifacts/*/layout_regression_summary.json`
