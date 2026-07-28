# 専用セッション指示書 索引

残る大きな deferred フェーズ/機能を、独立した集中セッションで安全に実施するための指示書一覧。各書は「背景/現在地・ゴール・難所・段階的アプローチ・検証・撤退ライン・関連ファイル」を含む。着手モデルはまず `saba/HANDOFF.md` 全体と該当指示書を読むこと。

近直の追跡タスク(fetch/XHR・CORS(+preflight)・プログレッシブ描画・watchdog・el.dataset・DOM 変異世代カウンタ・plan D5・frameset 複数 LivePage・Phase 4.1 の安全な最適化)は**完了済み**。以下は各々が独立した大きな作業:

| 指示書 | フェーズ | 概要 | 忠実度への効き | 規模/リスク |
|---|---|---|---|---|
| `phase-2.3-floats-bfc-brief.md` | 2.3 | float / clear / BFC | 高(サムネイル回り込み等) | 大・高(最難関) |
| `phase-2.5-inline-layout-brief.md` | 2.5 | インライン行ボックス本実装(UAX#14) | 高(折り返し全般) | 大・高(回帰面積最大、A/B 必須) |
| `iframe-rendering-brief.md` | 2.7発展 | iframe の実ドキュメント描画 | 中 | 大(ネスト browsing context) |
| `phase-0.8-dom-arena-brief.md` | 0.8/3.0 | DOM アリーナ移行(D1) | なし(内部) | 大・最リスク(純リファクタ) |
| `phase-4.1-incremental-layout-brief.md` | 4.1 | 真のサブツリー部分再計算 | なし(perf) | 大(安全網 `COSMO_LAYOUT_ASSERT` あり) |
| `phase-4.4-transition-animation-brief.md` | 4.4 | transition / @keyframes / animation | 中(動的表現) | 中〜大 |
| `phase-5-multiprocess-brief.md` | 5 | マルチプロセス化(ADR-0001) | なし(堅牢性/分離) | 大・最大(最終フェーズ) |

## 推奨着手順（更新: 2026-07-25）

> **重要な結合知見**: **2.3 floats と 2.5 inline は分離不可**に近い。float の回り込み(後続の利用可能幅短縮)は size パスに float コンテキストが要り、行単位の精密回り込みは 2.5 の行ボックスが前提。**float の部分実装は現状の描画を悪化させうる**(今 `float:right` は通常フローで重なり無し。エッジへ動かすだけだと後続が同 Y・全幅で重なる)。→ **2.3 と 2.5 は1つの結合作業として扱う**こと。`float`/`clear` の**プロパティパースは landing 済み**(`b184e07`)。

1. **4.4 transition/animation**（opacity/transform などレイアウト非依存プロパティ先行 = 最も安全に着手できる。まずここから）
2. **iframe**（frameset 基盤の要素単位一般化）
3. **4.1 incremental layout**（perf。安全網 `COSMO_LAYOUT_ASSERT` あり）
4. **2.3 + 2.5 結合**（floats + inline 行ボックス本実装を一体で。reftest 先行・A/B・回帰面積最大。最も慎重に）
5. **0.8 arena**（内部リファクタ。Boa 連携/4.1 dirty bit の土台。純リファクタ最リスク）
6. **5 multiprocess**（最終。前提の security.rs per-profile 化 + IPC v2 は前倒し可）
