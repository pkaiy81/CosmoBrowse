# CosmoBrowse — 作業引継ぎ (handoff)

最終更新: 2026-07-15 / ブランチ: `wip/native-renderer-winit-2026-03-30`

> **⭐ 2026-07-12: モダンブラウザ化の長期計画が承認された。今後の実装セッションは
> `docs/modernization-plan.md`(Phase 0〜5 のロードマップ)に従うこと。**
>
> **Phase 0 進捗(2026-07-13):**
> - 0.1 ✅ 死クレート削除(saba/src, net/wasabi, ui/wasabi, renderer-wasm, Tauriシェル)
> - 0.2 ✅ 脱 no_std(エンジン std 化、http.rs/url.rs 削除、`Page::load_html` に改称)
> - 0.3 ✅ 改名: **`cosmo_core_legacy`→`cosmo_engine`**(旧 cosmo_core シムの
>   paint_commands/paint_mapper/js_runtime を吸収)、**`cosmo_app_legacy`→`cosmo_runtime`**
>   (旧ラッパー解消、`SabaApp`→`BrowserApp`、コズミック別名全廃)。CI lint 追加、ADR-0003。
>   **テストは今後 `cargo test -p cosmo_engine` / `-p cosmo_runtime`**。
> - 0.4a ✅ 要素モデル開放: Element が実タグ名を保持(`tag_name()`)、未知タグは
>   `ElementKind::Unknown`(span 化廃止)。InBody が全開始タグを DOM 挿入(未知タグの
>   ドロップ廃止)、終了タグはタグ名で pop。型セレクタ/of-type は実タグ名比較。
>   HTML5 セマンティックタグ(nav/article 等)= block、meta/noscript 等 = 非描画。
>   **残: ua.css への既定スタイル抽出、video/canvas/svg プレースホルダ**
> - 0.5 ✅ layout_object.rs 分割(4209→2532行、純コード移動・スクショpixel一致):
>   `renderer/text/legacy_metrics.rs` / `renderer/style/{values,selector,cascade}.rs` /
>   `renderer/display/builder.rs`
> - 0.6 ✅ FontMetricsProvider: エンジンの全文字計測が provider 経由に
>   (`renderer/text/provider.rs`、未設定時は旧テーブル再現の Fixed でテスト期待値不変)。
>   renderer_native が fontdue 実装(`FontdueMetricsProvider`)を起動時に注入 —
>   **レイアウト計測と描画が同一フォント**に。`COSMO_LEGACY_METRICS=1` で旧挙動 A/B 可。
> - 0.7 ✅ テスト基盤: `scripts/run_layout_reftests.py`(reftest 6本+golden、--update で
>   再ベースライン)/ `testdata/sites/hn/`(HN 凍結ミラー)/ html5lib tree-construction
>   コーパスをベンダ+ランナー(`cosmo_engine/tests/html5lib.rs`、**現在 79/1592、
>   BASELINE_PASSES は上げるのみ**)。
> - おまけ修正: トークナイザが最終1文字を落とすバグ、InHead が body 前テキストを
>   捨てるバグ(コーパスが検出)。
> - **残: 0.8 DOM アリーナ移行(計画通り Phase 3.0 に回して可)**
>
> **Phase 1 進捗(2026-07-13):**
> - 1.1/1.2 ✅ **@media 対応**: `css/media.rs`(MediaQueryList AST+MediaContext 評価。
>   型/not/only/カンマOR/and連鎖/(min|max-)width/height/orientation/prefers-color-scheme、
>   未知 feature はそのクエリのみ不成立)。@media ブロックの規則は捨てずに文書順で保持し
>   `QualifiedRule::media` に条件 index を付与、`LayoutView::new_with_viewport` が
>   `StyleSheet::filter_for_media` で実 viewport 解決。**HN が 500px 幅で実物のモバイル
>   レイアウトになる**(desktop は pixel 一致=無回帰)。ダークモードは暫定
>   `COSMO_PREFERS_DARK=1`(winit テーマ接続は未)。@supports/@font-face/@import/
>   @keyframes は引き続きスキップ(1.1 残)。
> - 1.3 先行 ✅ font-weight(数値含む)/ visibility: hidden / **色関数+全named color**:
>   `parse_color_value`(style/values.rs)が hex・~150 named colors・rgb()/rgba()/
>   hsl()/hsla() を解釈、color/background(-color)/border(-color) に接続。
>   `color:inherit`→黒 と `background:url()`→白背景強制 の誤解釈も修正
>   (HN の votelinks 列が実物同様 beige に)。
>   さらに **min/max-width/height**(px/em/rem/%/none、compute_size 末尾でクランプ、
>   text と非置換 inline は除外)と **box-sizing: border-box**(resolved_width/height が
>   content 幅を返す方式。padding-only 境界は 1px 近似)。
>   **辺別 border longhand**(border-top/right/bottom/left(+-width)、border-style
>   none/hidden。SceneItem/DrawRect に serde-default の border_widths を追加=加算的
>   スキーマ変更、check_ipc_compat 通過。painter が辺別バー描画)も完了。
>   **vw/vh/vmin/vmax**(スタイルパス開始時に thread_local へ viewport を記録して実解決。
>   従来 100vh→100px だった)と **CSS の width/height %**(従来 CSS の `width:50%` は
>   完全に無視されていた!HTML width 属性のみ % 対応だった)も修正。
>   **list-style(-type)** ✅(disc/circle/square/decimal/none、継承、UA が ul→disc・
>   ol→decimal をシード、li が外側マーカー Text を発行、UA padding-left:40px)。
>   **white-space: pre/pre-wrap/pre-line** ✅(WhiteSpace enum 化、UA が `<pre>` に
>   Pre をシード、sizing/paint 共通の `build_text_lines`)。
>   残: プロパティレジストリ化(D2)、calc()、ch/ex、
>   inherit/initial/unset キーワード、辺別 border 色。
> **Phase 2 先行(2026-07-13):**
> - 2.1 第1弾 ✅ **単一行 flexbox**: flex/grow/shrink/basis(shorthand 含む)、
>   justify-content(center/end/space-between/around/evenly)、align-items/self
>   (stretch 既定・center・flex-end)、column/row-gap。行アイテムの主軸サイズは
>   テーブルセル方式(各アイテムが同じ分配を独立計算、`flex_row_main_size`)。
>   justify/align は `layout_flex_alignment` post-pass(translate_subtree)。
>   **注意**: compute_size 中の兄弟走査は自分が borrow 中 → `try_borrow` 失敗=自分、
>   というパターンで解決(flexbox.html reftest 参照)。
>   残: flex-wrap(複数行)、column 方向の grow 分配、% basis、order。
> - 2.2 第1弾 ✅ **grid-template-areas / grid-area** 配置(幅=エリア列スパン、
>   x=列プレフィックス、y=兄弟サイズから各自再計算する行高テーブル。コンテナ高=
>   エリア行の合計)+ **minmax()** トラック(max 部で解釈)。
>   残: grid-column/row の線番号形式、grid-template-rows、スパン付き auto-placement。
>   ~~Wikipedia シェル空白の真因未診断~~ → **解決**(下記)。
> - **Wikipedia 空白の真因2つを修正** ✅:
>   ① `collapse_text_whitespace` がタブ/CR/FF を潰さず、タブインデントの文書で
>   空白ノードが1行ずつ消費(数千行分!)。全空白文字を collapse(NBSP は除外)。
>   ② `height:0` が「未指定」扱いで無視され、`height:0;overflow:hidden` の
>   ドロップダウン(Vector のメニュー)が全展開。author 明示ゼロを cascade 時の
>   フラグで追跡し尊重(defaulting が全要素に 0.0 を入れるため is_some では不可)。
>   → **Wikipedia は1画面目からタイトル/タブ/infobox/本文が出る**ように。
> - **インラインのフロー終端カーソル** ✅: 折返しテキストの後続インラインが
>   「最大行幅×全行高の bbox 右上」でなく**最終行の続き**に配置される
>   (`text_last_line_width/line_count/line_height` を sizing 時にキャッシュ、
>   inline-inline 配置で参照。ゼロサイズ兄弟はアンカー規則どおりスキップ)。
>   リンク多用段落の行衝突が解消(inline_flow_cursor.html reftest)。
>   残: 内部で折り返す inline **要素**(bbox のまま)→ 2.5 の行ボックス本実装で。
> - 2.4 第1弾 ✅ **absolute の包含ブロック解決**: 最近傍 positioned 祖先の content box
>   基準(`absolute_containing_block`)。auto 辺は static 位置を維持(author フラグ)、
>   right/bottom 遠端アンカー対応。**margin/padding の辺別 longhand** も追加。
>   Wikipedia のドロップダウンが親の近くに配置されるように。
>   **既知のコスメ回帰**: HN の .hnname margin が(正しく)効いた結果、1024px で
>   nav の "submit" が2行目に折返す(行幅の過大見積り由来。計測改善で吸収予定)。
>   残: サイドバー断片の整理、検索ボックス位置。
> - **Wikipedia フィクスチャ凍結済み**(`testdata/sites/wikipedia/`、load.php CSS 2本を
>   ローカル化)。現状: 本文はスクロール位置(y≈4200)で**かなり読める**(段落・リンク・
>   見出し動作)。ブロッカー: ①Vector-2022 ヘッダシェルが flex/grid 未対応で縦積み
>   ~4000px(Phase 2.1/2.2)②一部段落で行の重なり(Phase 2.5 インライン書き直し)。
> - 次: 1.3 本体(プロパティレジストリ+box-sizing/min-max/border longhands/calc()等)
>   → 1.4 Web フォント → 1.5 :hover → 1.6 ディスプレイリスト刷新
>
> - **トークナイザ空白バグ修正(横断的)** ✅: タグ内の改行/タブが「タグ名や属性名の
>   一部」になっていた(区切りが半角スペースのみ)。複数行に整形されたタグ
>   (`<mdn-dropdown\n data-...>`)にセレクタが一切マッチしなかった。ASCII whitespace
>   全種を区切りに。**MDN の本文開始 y=27,403→3,915**(メガメニューが正しく畳まれた)。
> - **MDN フィクスチャ凍結済み**(`testdata/sites/mdn/`、CSS 17本ローカル化)。
> - **@media range 構文** ✅(`(width <= 1044px)`、値先行形・二重境界も)/
>   **calc()** ✅(絶対長の四則。var() 置換後に全プロパティ横断で px へ畳み込み。
>   % 混在は未解決のまま保持)/ **負の top/left** ✅(edge_to_i64 が 0 клипしていた
>   → 画面外退避イディオム `top:-20em` が動作)。
>   → MDN のトップナビタブ(HTML/CSS/JS/...)が横並びで出現。
> - **先頭ドット小数** ✅: CSS トークナイザが `.8em` を Delim(.)+8em に分解していた
>   (=10倍のpadding!)。数字が続く `.` を数値として消費。**MDN のタイトルが
>   1画面目に表示**されるように(y≈690)。
> - **guaranteed-invalid カスタムプロパティ** ✅: var() 置換を再帰化(深さ8制限)し、
>   `initial` を含む解決値は毒として伝播(フォールバック無しは番兵 initial を出力)。
>   csstools の light-dark() ポリフィル(MDN 採用)が正しく light 分岐を選択 —
>   **Baseline バナーがライトグリーン**に。
>   残ギャップ: ナビ行の乱れ、"Skip to search" 残存、タイトルまだ低め(y≈650)。
>   ナビタブのボックスが ~192px に膨張(ラベルは y=416 の 16px 一行)— 直接の
>   height/padding 指定は見当たらず。**容疑**: ①`display:contents`(mdn-dropdown/
>   navigation__popup)を inline 扱いしている(真の透過が必要)②`:is()` 未対応で
>   `.menu__tab-button:is(...)` 系が Never/誤マッチ ③::after chevron(mask 画像)の
>   空 content 箱。
> - **:is()/:where()/:matches()** ✅(コミット済み)— MDN は :is 多用のため効果絶大:
>   **タイトルが1画面目最上部**、ナビ1行化、Baseline バナー全文、記事リード、
>   "In this article" TOC まで描画。HN pixel一致・10 reftest 緑。
> - **名前付きグリッド線** ✅: `[name-start]` ブロックをトラック列と別テーブルに
>   パース(従来は幽霊 auto トラック化して列数を破壊)。`grid-area:X` が
>   X-start/X-end 線で解決。
> - **display:contents の透過** ✅: `DisplayType::Contents`(装飾/余白ゼロの全幅ブロック)。
>   配置ヘルパ(parent_grid_info/grid_area_rect/grid_area_row_heights)が
>   `effective_placement_parent` で contents ラッパーを透過して実グリッド祖先に解決。
>   MDN が `.layout__content{display:contents}` を使うため、**左サイドバー/本文/右TOC の
>   3カラムが分離**し重なり解消。MDN の flex 記事ページはかなり読める水準に。
 - **out-of-flow はフローアンカーにしない** ✅: `calculate_node_position` が
>   position:absolute/fixed の box を次兄弟のフローアンカーにしていた(zero-size のみ
>   除外だった)。fixed banner(top:0)や画面外 absolute(top:-20em)がページ全体を
>   上下にずらしていた。→ **MDN のタイトルがナビ下に正しく配置、3カラム整列。実物に近い。**
>   positioned.html golden 再ベースライン(fixed banner が .tall を押し下げなくなった)。
 - **`<?...>` bogus comment 破棄** ✅: TagOpen が `<!` は処理するが `<?` を素通しし、
>   `<?>`/`<?xml?>` の `?` と `>` が本文に漏れていた。仕様通り `>` まで破棄。
>   MDN の spec タイトルの「?>」断片が消えた。
> - **linear-gradient 背景** ✅(plan 1.6): `parse_linear_gradient`(角度 deg/`to <side>`、
>   色 stop の % 位置対応)。SceneItem/DrawRect に serde-default の background_gradient
>   =(angle, [(hex,pos)])を貫通(check_ipc_compat 通過)。painter が tiny-skia
>   LinearGradient で solid 背景色の上に描画(gradient 線は box 全体中心、長さ
>   |w·sinθ|+|h·cosθ|、stop の不透明度は要素 opacity)。gradients.html reftest。
> - **8桁/#rgba hex** ✅: `Color::from_code` が #rrggbbaa/#rgba 対応(painter は既に
>   alpha byte 対応済み)。`linear-gradient(#ffffff00, #000)` が下地色→黒にフェード。
> - **grid-template-areas が列数を規定** ✅(回帰修正): areas の列数 > 明示トラック数
>   の grid(Wikipedia の `areas:'columnStart pageContent'` + `columns:minmax(0,1fr)`)で
>   2列目が範囲外の幅0トラックに落ち本文が右端に潰れていた。不足トラックを auto 補完。
>   **Wikipedia のタイトル/タブ/infobox/本文がナビ下の正位置に復帰**。
>   残: 列幅がまだ約50/50(minmax の min サイジング未対応)、本文列の行重なり(2.5)。
>   注: Wikipedia 1280px の2列は `@media (min-width:1120px)` で正しい。列幅配分は
>   grid track sizing §11(minmax min-content・auto の正確な扱い)本実装が必要=大作業。
> - **text-transform** ✅(uppercase/lowercase/capitalize、継承): `display_text()` が
>   collapse 後に変換、sizing/paint 両方で使用(計測一貫)。
>
> **⭐ Phase 3 着手(2026-07-15):**
> - 3.1 第一歩 ✅ **cosmo_script クレート**新設(`boa_engine = "0.20"`)。`ScriptHost`
>   が Boa をラップ(Boa の API を他クレートに露出させない)。eval が算術/文字列/
>   制御フロー/再帰(fib(10)=55)/eval 間の状態保持で動作 — **玩具インタプリタ
>   (cosmo_engine::renderer::js)では不可能だった水準**。boa はクリーンビルド +約1分。
>   **まだどのクレートも cosmo_script に依存していない**(既存に影響なし)。
> - 3.2 第一歩 ✅ **DOM バインディング基礎**: cosmo_script が cosmo_engine 依存、
>   `document.getElementById` を公開。DOM は Boa GC 外の `Rc<RefCell<Node>>` のまま
>   (D5)、native fn は `ScriptHost::set_document` がセットする thread_local から読む
>   (Boa closure は Trace 必須、Rc DOM は未実装のため捕獲不可)。`collect_text` を pub 化。
>   **JS が実 DOM を読める**(getElementById('greeting')=='Hello DOM'、`.length`==9)。
>   現状 getElementById は textContent 文字列を返す簡易版。
> - 3.2 本命 ✅ **Element ラッパー + DOM 変更**: getElementById が実 Element JsObject を
>   返す。`NodeHandle`(`#[unsafe_ignore_trace] Rc<RefCell<Node>>`)を Boa の host data に
>   格納(boa_gc 0.20 の Trace/Finalize derive 用に依存追加)。`textContent` accessor の
>   getter=テキスト読取、setter=子を Text ノードで置換。`this` から downcast_ref で handle 復元。
>   **JS が実 DOM を変更できる**(`getElementById(id).textContent = ...` → Rust から読み戻して確認)。
>   Boa host data パターン確立: `JsObject::from_proto_and_data(None, data)` +
>   `insert_property(key, PropertyDescriptor::builder().get/set(...).build())`。
 - 3.2 属性 ✅ **Element の属性 API**: id/className accessor(get/set)、tagName(読取・大文字)、
>   getAttribute/setAttribute/hasAttribute メソッド。すべて NodeHandle host data 経由。
>   cosmo_engine に `Element::set_attribute`(+`Attribute::from_name_value`/`set_value`)と
>   `Node::kind_mut`(script からの in-place 属性変更用)を追加。
> - 3.2 querySelector ✅ (`5746ff2`) **document.querySelector / querySelectorAll**:
>   `dom/api.rs` に `query_selector[_all]` を追加(`CssParser` で `sel{}` をパースし
>   DOM を pre-order 走査、`dom_node_selected` でマッチ = スタイル解決と同一のマッチャ)。
>   querySelectorAll は JsArray を返す。子孫結合子・id/class・セレクタリスト対応。
> - 3.2 classList ✅ (`32f5a0d`) **element.classList** add/remove/toggle/contains。
>   同じ NodeHandle を持つトークンリストオブジェクトを返し class 属性を直接変更。
>   toggle は結果の所属を返し、任意の force 引数を尊重。
> - 3.2 ツリー変更 ✅ (`f09b2e8`) **createElement/createTextNode + appendChild/
>   removeChild/insertBefore/remove**: `Rc<RefCell<Node>>` の first/last-child・
>   sibling・parent リンクを保守しつつ splice。`detach_node`/`append_child_node`/
>   `insert_before_node` ヘルパ。textContent は Text ノードを in-place 読み書き。
> - 3.2 ナビゲーション ✅ (`24df18a`) **parentNode/parentElement・firstChild/lastChild・
>   nextSibling/previousSibling・children(要素のみ)・childNodes(全)**。
> - 3.3 イベント ✅ (`f444530`) **addEventListener/removeEventListener/dispatchEvent**:
>   ノード identity(`Rc::as_ptr`)でキーする LISTENERS レジストリ(D5: DOM 外に保持、
>   `set_document` でクリア)。Event は type/target + preventDefault/stopPropagation
>   (EventFlags host data)。dispatch は target→祖先(bubble)、stopPropagation で停止。
>   `ScriptHost::dispatch_event(node, type) -> bool`(false = default 抑止)で runtime が
>   実入力を注入可能。**capture フェーズはまだ未実装**。
> - 3.1/3.4 イベントループ ✅ (`4520f0f`) **console.\* + setTimeout/setInterval**:
>   console.{log,info,debug,warn,error} を CONSOLE_LOG にバッファ(`take_console_log`)。
>   setTimeout/setInterval/clearTimeout/clearInterval は仮想クロックのタイマーキュー。
>   `ScriptHost::run_pending(max)` が Boa の microtask job を回してから due タイマーを順に
>   発火(delay は順序だけ、ブロックしない)。ネスト setTimeout も解決。navigation でリセット。
> - 3.2 matches/closest ✅ (`89bc307`) **element.matches/closest + 要素スコープ
>   querySelector(All)**: `dom/api.rs` に `element_matches`/`element_closest`(祖先を遡上、
>   同じマッチャ再利用)。**loader.rs の注入ナビゲーションスクリプトが依存する closest('a') を解禁。**
> - 3.2 innerHTML ✅ (`88e8ac4`) **innerHTML get/set + getElementsByTagName/ClassName**:
>   getter は子を HTML シリアライズ(void 要素は閉じタグなし)、setter はフラグメントを
>   フルドキュメントとしてパースし body の子を target へ再ペアレント。collections は
>   query_selector_all 経由(`*`・空白区切り class 複合対応)。
 - window/location ✅ (`59c2012`) **window(=globalThis エイリアス)+ location**
>   (href/protocol/host/hostname/pathname/search/hash、`ScriptHost::set_location`)。
> - localStorage ✅ (`4840965`) **localStorage**(getItem/setItem/removeItem/clear/key/length、
>   挿入順保持)+ `ScriptHost::local_storage_entries`/`set_local_storage_entries`(玩具の
>   replace_local_storage 配線に対応)。navigation では非クリア(オリジン単位)。
> - E2E ✅ (`af8256a`) **TodoMVC 相当の統合テスト**(create/append/addEventListener/
>   dispatchEvent bubble/classList.toggle/removeChild/querySelectorAll の合成動作)。
> - postMessage/doc events ✅ (`25e5a2a`) **window.parent.postMessage / window.postMessage**
>   (JSON 直列化して POSTED_MESSAGES、`take_posted_messages` で drain)+ **document 直下の
>   addEventListener/removeEventListener/dispatchEvent**(root ノードに登録=バブルが届く)。
>   loader.rs 注入スクリプトの closest('a')→preventDefault→postMessage を E2E テストで再現。
>   → 差し替え前提 (b) 解決。
> - **cosmo_runtime 統合 ✅ (`c0d0e31`, 3.5 の主要部)** — `layout/mod.rs`
>   `build_layout_scene_with_script_runtime` が `COSMO_USE_BOA=1` で `cosmo_script::ScriptHost`
>   経由(set_location→set_local_storage_entries→set_document→<script> eval→run_pending→
>   localStorage 永続化→console を diagnostics へ)。**既定は玩具のまま**(凍結フィクスチャの
>   golden 不変)。両経路とも同 Rc<RefCell<Node>> を破壊的更新するので layout は post-script
>   ツリーを見る。暫定 512KB バイトキャップ=watchdog 代替(Boa 0.20 に fuel 無し)。
>   **cosmo_script に初の依存クレートができた。** テスト: `execute_scripts_boa` が <script> で
>   ノード追加→ツリー反映を確認(cosmo_runtime 52 tests)。
> - ラッパーキャッシュ ✅ (`7af23af`) **make_element を node identity(Rc::as_ptr)で
>   キャッシュ**→ `el === el` が query/navigation をまたいで成立、wrapper に付けた
>   カスタムプロパティも保持。キャッシュが Rc を pin するのでアドレス再利用なし。navigation でクリア。
> - capture フェーズ ✅ (`da7ff74`) **addEventListener の useCapture / {capture:true}**。
>   run_dispatch を capture(root→target親)→at-target(両方)→bubble(親→root)の3相に。
> - style ✅ (`c9d58e7`) **element.style**: setProperty/getPropertyValue/removeProperty/
>   cssText + camelCase アクセサ(backgroundColor→background-color 等、static テーブルを
>   captured closure で生成)。インライン style="" 属性を読み書き→エンジンが layout 時に
>   再パースするので次の relayout で反映。空値は宣言削除。
> - 次: (a) **COSMO_USE_BOA を GUI ヘッドレスで JS デモ検証**して golden 化 → 既定を Boa に
>   切替(専用コミットで再ベースライン、**ユーザー確認推奨**=凍結フィクスチャの回帰網に触れる)。
>   (b) Boa の実 watchdog(Context が !Send で別スレッド不可 → 反復/時間ガードの検討)。
>   (c) `renderer/js/` 玩具の削除。fetch/XHR(loader へワーカ委譲)、
>   DOM 変異世代カウンタ(全再構築でなく差分 relayout)、el.dataset、requestAnimationFrame。
>   cosmo_runtime の玩具 JS を cosmo_script に置換 + `renderer/js/` 削除、
>   MAX_SCRIPT_BYTES 撤廃、**DOM 変異→再レイアウトのトリガ**(現状 script は DOM を
>   変えるが再レイアウトされない)、innerHTML(フラグメントパース)、style(setProperty)、
>   setTimeout/setInterval、capture フェーズ、ラッパーキャッシュ(el===el)。
> **→ Phase 2 の受け入れ目標(MDN 記事)に到達。HN/abehiroshi/Wikipedia/MDN 全て読める。
>   残: Try it iframe 空欄(埋め込み未対応)、Wikipedia のサムネイル float 未対応。
>   次候補: 残 CSS(linear-gradient・transition)、floats(Phase 2.3)、
>   Phase 2.6 HTML パーサ(InTable/adoption agency/全実体参照)、Phase 3(Boa JS)。**
>
> 本書 §3 のビルド/テスト/スクショ手順(クレート名は cosmo_engine / cosmo_runtime に読み替え)
> と §7 のハマりどころは引き続き有効。
> 既知の未解決: wix 系ページで script テキストが本文に漏れる(トークナイザ RAWTEXT 未対応、
> Phase 2.6 で対処。0.4a 以前から存在)。

このセッションで、ネイティブレンダラ（winit GUI）を「実在のモダンサイトでハング/クラッシュせず、簡易なモダンサイトは描画できる」状態まで進め、さらに **#5（HN のテーブル列幅）を解決**した。HN は実物にかなり近いレイアウトで描画される。本書はその引継ぎ。

---

## 1. 現在地（コミット状況）

```
50425e4 feat(css): transform rotate() approximation                                 ← HEAD
b60fa48 feat(css): ::before / ::after generated content
24d4adf feat: white-space:nowrap + text-overflow:ellipsis, rounded border strokes
e21e20e feat: transform rendering, horizontal inner scrolling, border-radius + box-shadow
09a72b8 feat: inner scrollbars, hit-region clipping, opacity/transform stacking triggers
2cd87fe feat: interactive inner scrolling for overflow containers + clip inheritance
412a158 feat: sticky bottom bound, nested stacking contexts, negative z-index
5a83bdc feat: :not()/:*-of-type, inline baseline alignment, true position:sticky
69668c1 feat(layout): overflow scroll/auto, fixed right/bottom, inline vertical fixes
181dc61 feat: line-height, per-character text advances, position:fixed
5a93d7e feat(css): background-size, var() inheritance, inline-block, structural pseudo-classes
f3856a9 feat(css): attribute/sibling selectors, grid track sizes + gap, background-position
e1a273a feat(css): !important support
2709307 feat(css): selector specificity in the cascade
45df037 feat(css): real selector engine — lists, compounds, descendant/child combinators
193601a feat(layout): basic display:grid support
368fcba feat: bold width estimate, em/% font-size vs parent, CSS background-image + SVG
a2dcc24 feat(layout): inline whitespace separators, bgcolor precedence, cell box overhead
b0da10d feat(css): inline style attributes + continuous font sizes (pt/pc/in/cm/mm)
001caa2 feat(layout): fix HN table layout — legacy center reset + headroom column widths
165f548 feat(css): variables, comments, at-rule skipping, percentage table widths
5b90d56 feat(renderer): handle real-world sites without hangs + basic modern CSS
1339cec wip(native-renderer): layout, paint, and session improvements               ← 本作業の起点
```

- 作業ツリーはクリーン（追跡対象に未コミット変更なし）。
- 未追跡（コミット対象外。既存の作業用ファイル）:
  `saba/testdata/harakuku.html`, `saba/cosmo_core_legacy/testdata/`, `saba/scripts_tmp_bench.sh`
- 全テスト通過: `cosmo_core_legacy` 148, `cosmo_app_legacy` 51+3, `adapter_native` 等。
- 依存追加: `renderer_native` に `resvg`（SVG ラスタライズ。tiny-skia 0.11 と整合する 0.45）。

---

## 2. リポジトリ構成（要点）

ワークスペースは `saba/`（`saba/Cargo.toml`）。主要クレート:

| クレート | 役割 | 注意 |
|---|---|---|
| `cosmo_core_legacy` | レンダリングエンジン本体（HTML/CSS/JS/layout/paint）。saba本ベース | **`#![no_std]` + alloc**。std/eprintln/std::fs はコード本体でもテストでも使えない |
| `cosmo_app_legacy` | セッション/履歴・loader(HTTP取得)・layout統合・security | std。reqwest はここ |
| `adapter_native` | エンジンを native 向けに包む adapter（NativeAdapter） | |
| `renderer_native` | **winit GUI バイナリ本体**（`cosmo_browse_native`）。painter/text_render/ui_chrome | |
| `cosmo_core` | `pub use cosmo_core_legacy::* as nebula_renderer` 等の再エクスポート薄皮 | |

エンジンの再エクスポート名: `cosmo_core::nebula_renderer::...`（= `cosmo_core_legacy::renderer::...`）。

---

## 3. ビルド / 実行 / スクショ（重要）

```bash
cd /home/kkishimoto/work/CosmoBrowse/saba

# ビルド
cargo build -p renderer_native           # GUIバイナリ
cargo build                              # ワークスペース全体

# テスト
cargo test -p cosmo_core_legacy          # レイアウト/CSS/HTML/JS の単体テストはここに集中
cargo test -p cosmo_app_legacy           # 一部ネットワーク統合テストあり（~80s）

# ヘッドレス・スクリーンショット（GUIを開かずPNG出力。デバッグの主力）
BIN=target/debug/cosmo_browse_native
# 既存セッション復元の汚染を避けるため毎回別パスを指定すると確実:
export COSMO_SESSION_SNAPSHOT_PATH=/tmp/s.json; rm -f /tmp/s.json
$BIN --screenshot-wh <URL> /tmp/out.png <width> <height>
$BIN --screenshot-w  <URL> /tmp/out.png <width>           # 高さ既定
$BIN --screenshot    <URL> /tmp/out.png                    # 既定サイズ
# GUI起動: $BIN <URL>   （DISPLAY/wayland が要る。X11/wayland any_thread対応済み）
```

検証ティップ:
- `file://` は security により遮断される。ローカルHTMLは `python3 -m http.server <port> --directory <dir>` で配信して `http://127.0.0.1:<port>/...` を見る。
- ハング疑いは `timeout 60 $BIN ...` で囲む。
- `COSMO_DEBUG_DUMP=1` を付けてヘッドレススクショすると paint command の geometry（TEXT/RECT/IMG の x,y,w,h）が stderr に出る。実サイトのレイアウトデバッグの主力。
- no_std エンジンのデバッグは **テストハーネスからの println も std 不可**。値を見るには「テスト内でレイアウトツリーを walk して `panic!("...{:?}", ...)` で吐く」手が有効（本セッションで多用。例は git 履歴の dbg テスト参照、いずれも削除済み）。

---

## 4. このセッションで入れた変更（コミット別）

### `5b90d56` ハング/クラッシュ根絶 + モダン描画の土台
実在サイトのリンク遷移でハングしていた問題を多段で修正:
- **painter**: 画像取得を UIスレッド外（非同期）へ。遅い/到達不能なサブリソースでイベントループが固まらない。完了時に `EventLoopProxy<UserEvent>` で再描画を促す。`renderer_native/src/painter.rs`, `main.rs`。
- **JS lexer** (`renderer/js/token.rs`): 裸の `_` で無限ループ→OOM していたバグ修正（identifier-start/continue 不一致）。`contains` の境界外読み・予約語の word-boundary も修正。
- **JS runtime** (`renderer/js/runtime.rs`): 関数引数評価での `RefCell already borrowed` panic 修正。**実行 fuel** 予算（`MAX_EVAL_STEPS`）追加で非終端スクリプトを打ち切り。
- **HTML tokenizer** (`renderer/html/token.rs`): `<script>` 後に script-data 状態へ遷移（JSの `<`/`>` をタグ化しない）。終了タグは `</script>` のみ有効化。
- **CSS parser** (`renderer/css/cssom.rs`): EOF で無限ループする `while peek != '{'` を2箇所修正。
- **layout** (`renderer/layout/layout_object.rs`): テキスト行分割を **O(n²)→O(n)**（巨大テキストノードで数秒固まっていた `split_text` を一括走査に）。非描画要素（script/style/head/link/title）を `display:none` 既定化（`dom/node.rs` の `is_non_rendered_element`）。
- 基本 **flexbox**: `display:flex` / `flex-direction: row|column`。row はコンテンツ幅で横並び、column は縦積み。`computed_style.rs`(Flex/FlexDirection), `layout_object.rs`(max_content_width, sizing, positioning), `layout_view.rs`(テスト)。
- 未対応 `display` 値は不可視 `none` でなく可視 `block` にフォールバック。

### `001caa2` HN テーブルレイアウト解決（#5 完了）
3つの独立したバグの複合だった。**reverted だった two-pass 案とは別の解法**（`column_widths` をノードに保持する方式は不採用）:
1. **legacy center のセル内リセット** (`computed_style.rs`): `<center>`/`align=center` 由来の text-align:center が `<td>/<th>` 内容へ継承されてタイトルがセル内中央寄せになっていた。`text_align_legacy` フラグを追加し、legacy 由来の center はセル境界で start に戻す（CSS の `text-align:center` は従来通り継承）。
2. **列幅分配を CSS 2.2 §17.5.2.2 準拠に** (`layout_object.rs` `table_cell_auto_width`): 旧 SPACER(20)/CONTENT_SIZED(150) 閾値バケツを廃止。各 auto セルに min を保証し、余剰は **growth headroom（max-content − min）比例**で分配。rank 列「30.」は headroom 0 → 痩せたまま。タイトル列は `(github.com/anthropics)` のような長い非分割トークンで min≥150 でも「固定サイズ」誤分類されず成長できる（これが実 HN だけ崩れて単純再現で崩れなかった理由）。`column_max_hints` プリパス（`column_min_hints` のミラー）を追加。
3. **論理列マッピング** (`layout_object.rs`): `cell_column_index` と sibling-row 幅ルックアップが **colspan 分進む**ように修正。`<td colspan=2></td>` の後の subtext セルが物理 index 1（=votelinks 列 8px）を引いてセル幅 8px → テキストが1文字/行で縦積み → 行高 1000px+ になっていた。
- デバッグ補助: `COSMO_DEBUG_DUMP=1` でヘッドレススクショ時に paint command の geometry を stderr へダンプ（`renderer_native/src/main.rs`）。
- テスト: `test_hn_itemlist_column_distribution`（30行+spacer+More の忠実な HN 構造）追加。`test_tv_htm_long_row_wraps_within_cell` は外側セル rect のみ比較するよう堅牢化（handoff が予言していたブリトルさ）。

### `b0da10d` + `a2dcc24` モダン化第2弾（インラインstyle・実フォントサイズ・空白・bgcolor）
- **インライン `style="..."` 属性を解釈**（`cssom.rs` `parse_declaration_list` + `layout_object.rs` `create_layout_object`）。スタイルシートのルール適用**後**に cascade するので最優先で勝つ。HN の `height:5px` spacer 等が効くように。
- **連続フォントサイズ** `FontSize::Px(i64)` 追加（`computed_style.rs`）。CSS 数値 font-size が実ピクセルで効く（従来は 16/24/32 の3バケツで、24px 未満は全部 16px だった）。`length_to_px` に **pt/pc/in/cm/mm** 追加。テキスト計測は `char_width_px`/`line_height_px`（Px は線形スケール・**切り上げ**、レガシーバケツは従来比率のまま＝既存レイアウト不変）。
- **インライン間の空白保持**（`collapse_text_whitespace`）: 隣接 sibling が inline のときだけ先頭/末尾スペースを残す。空白のみノードは inline 間でスペース1個に。"197 pointsbyjenders" 連結バグ解消。
- **bgcolor の優先順位**: 自要素の `bgcolor` 属性を**継承より先**に適用（従来は `<table bgcolor>` の継承が先に埋めて自属性が無視→HN オレンジ帯が出なかった）。
- **セル box overhead**: `table_cell_auto_width` の配分予算からセル自身の padding/border を控除（返り値は content 幅で `compute_size` が padding を上乗せするため、行がテーブル右端からはみ出していた）。

### `24d4adf`〜`50425e4` nowrap/ellipsis・角丸枠線・生成コンテンツ・rotate（2026-06-13）
- **white-space:nowrap**（継承）で折り返し抑止、**text-overflow:ellipsis**でクリップ祖先の右端に合わせて `…` 切り詰め（`truncate_with_ellipsis`/`ellipsis_clip_width`）。
- **角丸の枠線**: border_radius>0 のとき4本の直線バーでなく**丸角パスをストローク**（クリップで欠けたらバーにフォールバック）。`border` ショートハンド/`border-color` が色をセットするように（従来は table の HTML border 属性のみ）。
- **`::before`/`::after` 生成コンテンツ**: 擬似要素セレクタを `Selector::PseudoElement(host, kind)` にパース（実要素には非マッチ＝漏れ防止）。`build_pseudo_element` がマッチ規則を specificity 順に集め、`content` 文字列があれば**インライン span+テキストの合成ボックス**を first/last child に挿入。content は文字列リテラルと none/normal対応。
- **transform: rotate()**（deg/rad/turn/grad）: ボックス中心+角度を回転コンテキストとしてサブツリーにスタンプ。painter が DrawRect 塗り（と丸角枠線ストローク）を中心回りに回転、mapper がテキスト/画像のアンカーを中心回りに回転（グリフは正立のまま＝近似）。no_std の sin/cos（範囲縮約 Taylor）。

### `e21e20e` transform描画・横内部スクロール・角丸/影（2026-06-12）
- **transform**: translate(px/%) はレイアウト後パスでサブツリー実移動（`translate(-50%,-50%)` センタリングイディオム動作）。scale は (origin, factor) をスタンプし **mapper がジオメトリ+フォントを一様スケール**（テキストも箱と一致して拡大）。absolute の **top/left % オフセット**対応（モーダルセンタの後半）。
- **横内部スクロール**: def に content 幅追加、オフセット (x,y) 化、ホイール横成分ルーティング、下端に横サム。**overflow コンテナ内の明示幅はクランプしない**（あふれた分がスクロール対象）。`COSMO_INNER_SCROLL="id:dx:dy"`。
- **border-radius**(単一半径): Bezier 角丸パスで fill（クリップで欠けた箱は角丸スキップ — 切断辺は角ではない）。**box-shadow**(dx dy blur color): 同心拡張×低アルファの擬似ブラーを背面に。モダンカードルックが出る。

### `09a72b8` スクロールバー・ヒットクリップ・opacity/transform トリガ（2026-06-12）
- **内部スクロールバー**: コンテンツがあふれる scroll コンテナの右端にトラック+サム描画（inner オフセット反映）。
- **ヒット領域クリップ**: リンクのヒット矩形をコマンドの clip と交差（完全クリップは破棄）。overflow で隠れたリンクが押せなくなった。
- **opacity<1 / transform≠none の stacking context**: 仕様どおり**通常フロー位置のまま** context を形成 — 自分は持ち上がらず、子孫の z context をバケツ内に閉じ込める（global ±1M へ脱出させない）。transform は描画未対応でトリガフラグのみ。

### `412a158` + `2cd87fe` sticky境界・stacking・内部スクロール（2026-06-12）
- **sticky 下端バウンド**: コンテキストが (top, y, **max_delta**=包含ブロック底−自box底) に。painter がクランプし、セクションを過ぎるとバーが解放される。
- **stacking 本対応**: レイアウト後パスが各ノードに `paint_z` を計算（ルートキャンバス −2M / 通常フロー 0 / context ±1M+z / ネスト context は親バケツ内オフセット）。**z-index:auto の positioned は context を形成しない**（コンテンツは持ち上がるが、子の z:-1 は周囲の context 基準 = Chrome 同等。content層/context基準の2チャネル継承）。**背景色の継承を廃止**（CSS では非継承。不透明コピーが負zレイヤを覆い隠していた）。painter のキャンバス判定は位置ヒューリスティック→ **paint_z マーカー（≤−1.5M）** に（HN オレンジ帯の全画面化事故の根治）。
- **内部スクロール**: クリップ継承（overflow 祖先の交差を `final_clip` としてスタンプ、**テキストもクリップ**: `draw_text_clipped`）。overflow:scroll/auto に id を採番しサブツリーへスタンプ、コンテナ自身は (id, content高) を運ぶ。renderer は per-container オフセット（クリップは静止）、GUI はカーソル下の最上コンテナへホイールをルーティング（端でページへフォールスルー）。`COSMO_INNER_SCROLL="id:px"` でヘッドレス検証。

### `5a83bdc` :not()/of-type・ベースライン整列・真の sticky（2026-06-12）
- **`:not(セレクタリスト)`**: 引数をセレクタ機構でインラインパース（specificity は引数のもの）。**`*-of-type` 族**: 同タグ兄弟のみカウント。
- **ベースライン整列**: レイアウト後パスで同一上端のインライン連続兄弟を行にまとめ、最深ベースラインへシフト。ascent 推定: テキスト=font px（描画は top+font_px がベースライン）/ インライン要素=padding-top+font px / 画像=全高（置換要素はベースラインに乗る）。
- **真の sticky**: 通常フロー配置のまま、レイアウト後パスで (top 閾値, 配置y) を**サブツリー全ノードの style にスタンプ**→ painter が `effective_scroll` でクランプし閾値で貼り付く。同パスで **fixed のサブツリー**にも `fixed_subtree` をスタンプ（従来は fixed 要素本体だけスクロール免除で、子はスクロールで流れた）。
- **stacking context をコマンドの z に合成**（mapper で ×1,000,000）+ engine の stacking 判定に sticky/fixed サブツリー所属を追加。これが無いと「ピン留めバーの背景が自分のテキストを塗りつぶす」「ページテキストがバーの上に描かれる」。
- **`COSMO_SCREENSHOT_SCROLL=<px>`**: ヘッドレススクショをスクロール状態で撮る（sticky/fixed の検証はこれで）。

### `69668c1` overflow scroll/auto・fixed right/bottom・インライン縦修正（2026-06-12）
- **overflow: scroll/auto**（+ overflow-x/-y）を hidden 同様クリップ扱い（内部スクロール操作は将来の renderer 課題。未スクロール表示としては正確）。
- **fixed の right/bottom**: `offset_right/bottom` をパースし、**レイアウト後パス** `reposition_fixed_far_edges` で viewport 遠端にアンカー（right はサイズ確定後でないと解決できない）→ サブツリーごと平行移動。`LayoutView::new_with_viewport` が viewport 高さを保持（0=不明なら bottom 無効）。app はフレーム高を渡す。
- **インライン縦の2バグ修正**（チップ食い込みの真因）:
  1. 同一行のインライン兄弟が prev.y + 自分の margin-top で**2pxずつ沈んでいく**（creep）→ 前の箱と上端揃えに。
  2. **サイズ0ノード**（ブロック境界で空に潰れた空白テキスト）が次兄弟のフローアンカーになり、後続ブロックが実際の行に重なって配置 → 位置決め走査がサイズ0ノードを素通しして実アンカーを引き継ぐ。

### `181dc61` line-height・文字別アドバンス・position:fixed（2026-06-12）
- **line-height**: 数値（自フォント倍率）/長さ/%/normal → `LineHeight` enum（継承）。行送りと `<br>` 高さが `styled_line_height` 経由に。
- **文字クラス別アドバンステーブル**（DejaVu Sans 16px 実測準拠: i/l≈5, 小文字≈10, 大文字/数字≈11, m≈16）で旧一律 8px を置換。**計測と `split_text` が同じ per-char 切り上げ集計**を使う（合計を一括スケールすると丸め差で "login" が "logi/n" に折れる）。インラインの重なり解消、HN ナビの字間が実物同等に。
- **position:fixed**: viewport 原点 + top/left 配置。`fixed` フラグを DrawRect/Text/Image と SceneItem に貫通（serde default）。painter は fixed コマンド（とヒット領域）をスクロールから免除。**ページキャンバス背景の引き伸ばしヒューリスティック（y=0・幅広）から fixed を除外**（fixed nav が全画面の塗りつぶしになる）。`sticky` は static 扱い（未スクロール表示では正しい）。

### `5a93d7e` background-size・var()継承・inline-block・構造擬似クラス（2026-06-11）
- **background-size**: 明示寸法（px/%/auto）と cover/contain。shorthand の `/ size` セグメントも抽出。painter は「描画サイズ」を解決してサンプリング（auto は比率維持、position の % は描画サイズ基準）。
- **var() の要素別カスケード**: `ComputedStyle` に Rc copy-on-write のスコープを保持し親から継承。要素の `--name` 定義が部分木にだけ効く。**ルート（body）は DOM 祖先（html）にルールを評価してシード** — `:root`（新 `PseudoClassKind::Root`、要素親を持たない要素にマッチ）は html にしか付かないが html は layout object を持たないため。グローバル前置換（`resolve_css_variables` 呼び出し）は app から削除。
- **inline-block**: 専用 DisplayType。インラインに流れ、明示 w/h 尊重、未指定なら max-content に shrink-wrap（containing block でキャップ）。
- **構造擬似クラス**: `:root`/`:first-child`/`:last-child`/`:only-child`/`:nth-child()`/`:nth-last-child()`。An+B マイクロシンタックス完全対応（odd/even/2n+1/-n+3、`2n-1` が `Dimension(2,"n-1")` に融合するトークン化も処理）。specificity は class 扱い。
- ハマりどころ: ルートの var() シードを「文書中の全 `--` 収集」にすると `.theme{--x}` の上書きまで混入する（テストが検出）→ DOM 祖先へのルール評価方式に。

### `f3856a9` 属性/兄弟セレクタ・gridトラック・background-position（2026-06-11）
- **属性セレクタ** `[name]`/`[name=v]`/`~=`/`|=`/`^=`/`$=`/`*=`（specificity は class 扱い）。**兄弟結合子** `+`/`~` 対応。
- **セレクタマッチングを DOM 木ベースに変更**（cascade 時点で layout object は親に未リンクなので兄弟ウォークは DOM でしか出来ない — ハマりどころ）。
- **grid トラックサイズ**: `grid-template-columns` が px/fr/%/auto/`repeat(N,...)` を実解釈（`GridTrack`）。`200px 1fr` が本当に 200px+残りに。**gap/column-gap/row-gap** 対応（位置は prefix-sum+gap、コンテナ高さに行間 gap 加算）。
- **background-position / background-repeat**: px・キーワード（%変換）両対応、shorthand からも抽出（関数引数と `/size` 部はスキップ）。DrawRect/SceneItem 経由で painter へ（serde default でワイヤ形式互換）。painter は統一サンプラ: position 指定＝スプライト切り出し（負オフセット可・no-repeat はクリップ・repeat は rem_euclid）、position 無し＝従来のタイル/fit。
- 派生修正: **トークナイザの負数**（`-16px` が Ident になりスプライトオフセットが消えていた）/ **inline 要素の明示 width/height**（inline-block 近似。子無し 16×16 アイコン span が 0×0 で消えていた）。

### `e1a273a` !important（2026-06-11）
- 宣言末尾の `!important`（大文字小文字無視）をパース時に剥がして `Declaration.important` フラグへ（値解釈への漏れ防止）。
- カスケードを **4段の重要度ティア**で適用（弱い順・後勝ち）: 通常ルール(specificity順) → 通常インライン → !important ルール(specificity順) → !important インライン。型セレクタの !important が #id 通常規則やインライン通常 style に勝つ。

### `2709307` セレクタ specificity（2026-06-11）
- マッチしたルールを **specificity 昇順（id, class, type を u32 にパック）+ 安定ソート**で適用。同値は文書順後勝ち、インライン style は従来通り最後＝最優先。
- **セレクタリストはパース時に展開**（`h1, .x{}` → (0,0,1) ルールと (0,1,0) ルール。specificity はリストでなく個々の複雑セレクタの属性）。
- 効果: HN の pagetop ナビが実物同様の**黒文字**に（`.pagetop a{color:#000}` が裸の `a` 色規則に勝つ）。

### `45df037` セレクタエンジン（2026-06-11）
旧パーサは1ルール1単純セレクタで、トークンごとに**セレクタを上書き**していた（`.admin td{}` が `td{}` になり全セルに適用 / `h1,h2{}` は h2 のみ）。これを本物のセレクタエンジンに:
- **トークナイザ**: 空白・コメントを `Whitespace` トークンとして発行（`.a .b` と `.a.b` の区別に必須）。セレクタ以外の消費側は全てスキップ。**ルートのルールループでもスキップ必須**（怠ると空白の陰の at-rule が見えず @media ブロックが漏れる — 既存テストが検出）。
- **AST**: `List`(`,`) / `Compound`(`div.a.b`) / `Descendant`(空白) / `Child`(`>`) / `Universal`(`*`) / `Never`。`:hover` 系・擬似要素(`::before` 等)・兄弟結合子(`+`,`~`)・属性セレクタは **Never に毒化**（過剰マッチで装飾styleが漏れるより安全）。`:not(..)` 等の引数は括弧バランスでスキップ。
- **マッチング**: class は**複数クラス属性対応**（`class="athing submission"` に `.athing` がマッチ — 従来は完全一致のみ！）。Descendant は祖先チェーン走査。
- **副作用の修正**: inline 要素の幅にブロック子の outer 幅を算入（`<a><div class=votearrow></div></a>` が幅0になり矢印がタイトル側にはみ出した。旧描画は誤パースの `table.padtab td{padding:0 10px}` が全 td に漏れて偶然成立していた）。
- 効果: **text.npr.org の積年の行重なりが解消**（npr の CSS は子孫セレクタ前提）。HN の行間が実物同等のコンパクトさに。

### `368fcba` + `193601a` モダン化第3弾（太字幅・em/%・背景画像SVG・grid）
- **太字の幅推定** `bold_width_adjust`（×1.125 切り上げ）を全テキスト計測に適用。太字の後続インラインが重ならない。
- **font-size の em/% を親基準で解決**（`cascading_style` が親フォントサイズを受ける）。CSS トークナイザが `%` を `Dimension(n,"%")` として出すように（従来 `font-size:80%` が 80px 扱いだった）。
- **`background(-image): url(...)`** を style に取り込み（`extract_css_url`、引用/非引用/多層対応）。`min_content_width_hint` が子の **CSS width+margin** を数える（votelinks セルが 0 幅に潰れて votearrow が消えていた）。renderer は **resvg で SVG をラスタライズ**（premultiplied→straight 変換必須）。rect より大きい背景画像は**スケールして fit**（タイルの左上クロップだと triangle.svg の空白角だけ見える）。HN の votearrow ▲ が描画されるように。
- **`display:grid` 基本対応**（`193601a`）: `grid-template-columns` のトラック数だけ解釈（`repeat(N,..)` 対応、**全トラック等幅**）。要素子を row-major 配置。空白テキスト子は grid item ではない（`grid_item_index` でスキップ。これを怠るとカードが互い違いになる）。

### `165f548` CSS変数・コメント・at-rule・パーセント幅
- **CSS変数** `var(--x[, fallback])`: パース後に `:root` 等の `--name` を文書全体マップに集約して置換（`cssom.rs` の `resolve_css_variables`、`layout/mod.rs` で呼ぶ）。色/フォールバック/ネスト対応。
- **CSSコメント** `/* */`: トークナイザでスキップ（`css/token.rs`）。**これが HN崩壊の主因だった** → `/* mobile */ @media{...}` のコメントで `@media` を検出できず、中身が通常ルール化して `display:block`/`width:100%` 等が漏れていた。
- **at-rule スキップ**: `@media`/`@supports`/`@font-face`/`@import` をブロックごと破棄（`cssom.rs` の `consume_at_rule`）。条件は評価できないので捨てるのが安全。
- **パーセント幅**: `<table width="85%">` / 入れ子 `width="100%"` を含有幅基準で解決（`layout_object.rs` の `html_width` を `parse_dimension_pct_attr` に）。入れ子auto表の崩壊を解消（HNヘッダ修正）。

---

## 5. 動作確認できているサイト

| サイト | 状態 |
|---|---|
| example.com | ✅ ほぼ正確（中央寄せ・背景・リンク） |
| lite.cnn.com | ✅ 見出しリンク一覧が読める |
| text.npr.org | ✅ きれいに読める（`45df037` で行重なり解消） |
| Hacker News | ✅ **実物にほぼ一致**。オレンジ帯・beige背景・votearrow ▲・10pt/7ptフォント・スペース込み subtext。残: ヘッダ太字 "Hacker News" と "new" の間がまだ僅かに窮屈 |
| Grid デモ（3カラムカード+2トラック） | ✅ row-major 配置。トラックは等幅のみ（`200px 1fr` も 50/50 になる） |
| 構成テスト(flex nav/カード/縦並び) | ✅ Chrome 同等 |
| abehiroshi（frameset+table・レガシー） | ✅ きれいに描画（回帰なし） |
| harakuku.com / tbs.co.jp (VIVANT) | ✗ ほぼ空白。重い装飾（sprite背景・絶対配置・grid・text-indent等）が未対応。**現エンジンの範囲外** |

方針メモ: モダン重量級サイトは「外部CSSを部分適用すると、未実装のレイアウト機構（flex/grid/absolute/overflow/背景sprite）前提のため逆に崩れる/隠れる」。**簡易なモダンサイトを対象に段階的拡張**していく合意。

---

## 6. 未完了タスク

### #5 table 列幅の精緻化 — ✅ **完了**（`001caa2`）
解法は §4 の `001caa2` 参照。過去に revert した two-pass `column_widths` 保持案は不要になった（headroom 比例分配＋論理列マッピング修正で解決）。

### 完了済み（このセッション）
太字幅推定 ✅ / em・% の親基準解決 ✅ / background-image url() + SVG（votearrow）✅ / display:grid 基本 ✅（いずれも `368fcba`+`193601a`）

### その他の候補（未着手）
- **`<label>` 等の未知要素**（label/section/article/nav 等が ElementKind 未登録 → class/display が効かない。HTMLパーサの要素表拡張）。
- transform の **skew/matrix**、回転時の**子要素の正確な変換**（現状アンカーのみ回転、グリフ正立）。
- `content` の url()/counter()/attr()、`::first-line`/`::first-letter`。
- CSS グラデーション（`linear-gradient`）の背景描画。
- 関連: `cosmo_app_legacy/src/layout/mod.rs` の `MAX_SCRIPT_BYTES=32KB` ガードは、lexer無限ループ・実行fuel を入れた今は緩めても安全（要検討）。

---

## 7. ハマりどころ / 設計メモ

- **no_std**: `cosmo_core_legacy` は `#![no_std]`。`std::env`/`std::fs`/`eprintln!` 不可（テストでも）。デバッグは panic ダンプか、`cosmo_app_legacy`(std側)に寄せる。
- **レイアウトは 2 パス**: `calculate_node_size` が各ノードに `compute_size` を**2回**呼ぶ（子の前=トップダウン幅確定、子の後=ボトムアップ高さ/shrink-to-fit）。table 列ヒントは pre-pass で 1 回計算してノードに保持。
- **インライン `style="..."` 属性は解釈される**（`b0da10d` 以降。スタイルシート後に cascade、最優先）。
- **フォントサイズは連続値**（`FontSize::Px`）。ただし `em` は cascade 時 16px 基準（rem と同義）。h1-h3 既定やキーワードはレガシーバケツ（16/24/32）のまま。
- **外部CSS**は `cosmo_app_legacy/src/loader.rs` の `fetch_external_stylesheets`（URLキャッシュ・サイズ上限・http(s)限定・失敗スキップ）。`layout/mod.rs` で inline と結合→`resolve_css_variables`→`CssParser`。relayout時はキャッシュ利用。
- **画像は非同期**（painter）。スクショ（ヘッドレス）モードでは同期取得にフォールバック（notifier未設定時）。
- HN は時々エンジンの取得をブロックする（bot対策）。ローカルにミラー（HTML+news.css を取得し `href="news.css"` に書換え）して `http.server` 配信で再現するのが安定。
- セッションスナップショットが復元されて前回URL群がレイアウトされる→スクショに混じる。`COSMO_SESSION_SNAPSHOT_PATH` を都度別パスにして回避。

---

## 8. すぐ再開するなら

```bash
cd /home/kkishimoto/work/CosmoBrowse/saba
cargo build -p renderer_native && cargo test -p cosmo_core_legacy   # 緑であることを確認
# 例: HNミラーを作って描画確認（残課題は §6）
mkdir -p /tmp/hnmirror
curl -sSL https://news.ycombinator.com/ -o /tmp/hnmirror/index.html
curl -sSL https://news.ycombinator.com/news.css -o /tmp/hnmirror/news.css
sed -i -E 's#href="news\.css[^"]*"#href="news.css"#' /tmp/hnmirror/index.html
python3 -m http.server 8806 --directory /tmp/hnmirror &   # 別ターミナル推奨
COSMO_SESSION_SNAPSHOT_PATH=/tmp/s.json ./target/debug/cosmo_browse_native \
  --screenshot-wh http://127.0.0.1:8806/index.html /tmp/hn.png 1024 1200
```

タスク一覧: #1 外部CSS ✅ / #2 example.com ✅ / #3 HN（読める）✅ / #4 flexbox ✅ / #5 table精緻化 ✅（`001caa2`）/ #6 var() ✅。次の候補は §6「HN 残課題」か「その他の候補」から。
