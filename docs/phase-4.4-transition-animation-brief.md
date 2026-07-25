# 専用セッション指示書: Phase 4.4 「transition / @keyframes / animation」

> 独立セッション用。着手前に `saba/HANDOFF.md` 全体と本書を読むこと。フレームクロック駆動が鍵。

> **✅ JS アニメーション駆動は実装済み(`851295a`, 2026-07-25)**: `ScriptHost::run_frame`(1フレーム分だけ仮想時間を進めて due タイマー/rAF を発火)+ `has_pending_timers`、`LivePage::{has_pending_animation, animation_frame}`、`AppBridge::animation_frame`、`renderer_native::about_to_wait`(`ControlFlow::WaitUntil` で ~60fps 駆動、アイドル時待機)。ヘッドレスは settle で有限アニメを完走。**rAF ループ/setInterval で style を変えるアニメは GUI で動作**(60フレームのバー成長を検証)。**本書の残りは「宣言的 CSS transition / @keyframes animation」**(下記)。static ページは非回帰(active timer 無し)。

## 背景 / 現在地

- `@keyframes` は現状 **プレリュード/中身ともスキップ**(`cosmo_engine/src/renderer/css/cssom.rs:570` 付近のコメント参照)。at-rule 保持基盤はあるが keyframes は未保存。
- `transition` プロパティは未対応。
- `requestAnimationFrame` / `cancelAnimationFrame` は `cosmo_script` 実装済み(`ScriptHost::run_pending` の仮想クロックで駆動)。ただし GUI の実フレームクロックには未接続(load 時の run_initial_load は rAF を1ラウンドのみ)。
- プログレッシブ描画(`LivePage` + waker + `pump_and_relayout`)と `COSMO_LAYOUT_ASSERT` 安全網あり。

## ゴール

- **transition**: 対象プロパティ(length/color/opacity/transform)を、指定 duration/easing で補間。トリガ(hover/class/style 変更)で開始。
- **@keyframes 保存 + animation 再生**: `@keyframes` を CSSOM に保存し、`animation-*` で再生。
- **フレームクロック**: イベントループのフレーム時計で rAF/補間を駆動。GUI は winit のフレーム/タイマーで定期 pump。

## ✅ 宣言的 CSS transition ドライバ（`6fa220d`, 2026-07-25 landing 済み）

下の設計どおりに実装完了。**opacity の CSS transition が GUI で補間される**:
`ComputedStyle::{anim_opacity, used_opacity}` + `LayoutView::collect_transition_targets()`(エンジン)、
`cosmo_runtime/src/layout/transitions.rs` の `TransitionDriver` + `LivePage::animation_frame` 駆動。
override は `data-cosmo-anim-opacity` として DOM 上にあるので **full レイアウトがアニメフレームを再現
= `COSMO_LAYOUT_ASSERT` がアニメ中も成立**。検証: fetch ハンドラの class 付与で 10s transition が
ヘッドレススクショで中間状態(≒52%)、200ms は完走。reftest 12/12。

`aaca6b4` でドライバは**プロパティ汎用**に(`AnimatedProperty`/`AnimatedValue`、キーは (node, property))、
**background-color** も補間対象。プロパティ追加は variant + `used_*` アクセサだけ。

**この節の残り**: ① color(継承あり)/transform の補間② length 系(relayout が要る)
③ **`run_initial_load` が pending タイマーを全消化**するため `setTimeout` 起点の class 変更は
初回描画前に確定してアニメしない(ロード後の fetch/XHR・フレームクロックのタイマーは動く)。
④ `:hover` 起点(1.5 未実装)。
~~⑤ クリック等の実入力を LivePage へ dispatch~~ → **完了 (`ecdc07a`)**: 実クリックが JS に届き、
ハンドラの class 変更から transition が起動する(GUI 検証済み)。

### 当初の設計メモ（実装済み・参照用）

`transition` の**パースと easing コアは landing 済み**(`f798097`: `ComputedStyle::transitions()`/`transition_for()`、`Easing::apply(t)`)。**残るのはドライバ**(目標値変化の検知→補間→適用)。設計:

1. **要素↔計算後スタイルの橋渡し**: `LayoutObject` は `node: Rc<RefCell<Node>>` を持つので、`LayoutView` に「各要素の (DOM ノード, 計算後 opacity, opacity の transition 設定)」を返す `collect_transition_targets()` を追加。**target は cascade 由来**(override を含めない)を返すこと。
2. **override は cascade と分離**: LivePage が補間値を `data-cosmo-anim-opacity` 属性としてノードに設定し、**エンジンの paint はこの属性があれば opacity をそれで上書き**(cascade の計算後 opacity は「目標」として残す)。これで「目標(stylesheet) vs 適用値(override)」が混ざらない。
3. **LivePage の transition トラッカ**: `HashMap<node_ptr, ActiveTransition{ from, to, start_ms, dur, easing }>` + `last_target: HashMap<node_ptr, f64>`。relayout 後に `collect_transition_targets` を呼び、target が last と変われば「現在の表示値→新 target」の transition を開始。
4. **駆動**: `LivePage::animation_frame` で経過 ms を進め、各 active transition の補間 opacity を `data-cosmo-anim-opacity` に書き、relayout+repaint。`has_pending_animation` に「active transition あり」を含める(タイマー同様にフレームクロックが回る)。完了で属性除去・状態削除。
5. **拡張**: color/transform も同様(color は補間、transform は translate/scale/rotate の数値補間)。まず opacity で成立させてから。

**検証**: `transition: opacity .3s` の要素に JS で class を付けて opacity を変え、GUI(または settle でフレーム完走)で中間/最終状態が滑らかに変わることをスクショ確認。static ページは transition 無しで非回帰(reftest 12/12)。

## 難所

1. 補間には「前フレームの computed 値」と「現フレームの目標値」が要る → LivePage が要素ごとのアニメ状態を持つ必要。
2. GUI の連続フレーム駆動: 現状 pump は fetch/timer 完了時のみ。アニメ中は winit で ~60fps の定期再描画(`ControlFlow::WaitUntil` or 定期 UserEvent)が要る。
3. 部分再計算が無い今、アニメ毎フレーム full relayout は重い(Phase 4.1 と相性)。まずは opacity/transform など**レイアウト非依存**プロパティを scene 差分だけで更新するのが軽い。
4. easing 関数(cubic-bezier)、animation-iteration/direction/fill-mode。

## 推奨アプローチ（段階的）

1. **@keyframes 保存**: `cssom.rs` で `@keyframes name { 0%{...} 100%{...} }` をパースして保持(スタイル解決からは Phase 4.4 で参照)。
2. **transition パース + 状態**: `transition` プロパティ解析、要素ごとに「開始値・目標値・開始時刻・duration・easing」を LivePage が保持。
3. **フレームクロック**: GUI がアニメ実行中は ~16ms ごとに `pump`(= 補間を進めて scene 更新)+ 再描画。winit の `ControlFlow::WaitUntil` または waker の定期発火。`ScriptHost` に「実時間を進める」API を追加(現状は仮想クロック)。
4. **レイアウト非依存プロパティ先行**: opacity/transform/color は relayout 不要 → scene 差分だけ更新(Phase 4.1 無しでも軽い)。length 系は relayout 要。
5. **rAF 接続**: フレームクロックで rAF コールバックを毎フレーム発火(requestAnimationFrame デモ動作)。

## 検証

- `cd saba && cargo test`。
- transition/animation のデモページ(hover で色/opacity/transform が補間)を GUI(または連続 pump のヘッドレス計測)で確認。
- 全画面再描画なしで動く(ダメージ矩形は Phase 4.3、なければフル再描画で可)。
- reftest 12/12 維持(静的ページはアニメ無しで不変)。

## 撤退ライン

フレームクロック統合が重い場合、**transition の opacity/transform/color(レイアウト非依存)を rAF 駆動で補間**するところまで landing。length 系 transition と @keyframes animation は追補。GUI の定期フレームは最小実装(アニメ中のみ WaitUntil)。

## 関連ファイル

- `saba/cosmo_engine/src/renderer/css/cssom.rs`(@keyframes 保存)、`style/cascade.rs`(transition/animation プロパティ)。
- `saba/cosmo_script/src/lib.rs`(rAF、実時間クロック API 追加)。
- `saba/cosmo_runtime/src/layout/mod.rs`(`LivePage` にアニメ状態 + フレーム pump)。
- `saba/renderer_native/src/main.rs`(winit フレームクロック / `ControlFlow::WaitUntil`)。
