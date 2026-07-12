# CosmoBrowse モダンブラウザ化 — 設計と実装計画

## Context

CosmoBrowse は saba 本由来の自作 Rust ブラウザエンジン。現状は HN / abehiroshi 級の簡易ページは実物に近く描画できるが、近年の Web ページは崩れるか白紙になる。原因は構造的: `@media` 全捨て・Web フォントなし・手打ち文字幅テーブル(実フォントメトリクス不在)・flex/grid が初歩・float なし・閉じた ElementKind enum(未知要素が inline span 化)・玩具 JS(if/for すら無い)・32KB スクリプト上限。

本計画は「近年の Web ページも表示できるモダンブラウザ」への段階的進化のロードマップ。**実装は別モデル/別セッションが担う**前提で、フェーズごとに作業項目・対象ファイル・受け入れ基準を明記する。

### ユーザー決定済み方針(再交渉しない)

1. **レンダリングエンジン(HTML/CSS/レイアウト/ペイント)のロジックは完全自作を継続**。html5ever/stylo/taffy/servo 部品は使わない。
2. ただし**データ/デコード系はクレート可**: フォントバイナリ解析(ttf-parser/fontdue)、Unicode データ表(unicode-linebreak/unicode-segmentation、後に unicode-bidi)、WOFF 展開用圧縮(flate2/brotli)、WHATWG entities.json・html5lib-tests はデータとして利用。
3. **no_std は廃止**(Wasabi OS ターゲットは死んでいる)。std/HashMap/f64 数学/外部クレートを解禁。
4. **JS だけは既存エンジン統合**: Boa(純 Rust)を採用し玩具インタプリタは削除。
5. **直近(~3ヶ月相当)目標: 静的〜軽JSのモダンサイト**(Wikipedia/MDN/ブログ/ニュース)がレイアウト崩れなく読める。SPA は後続フェーズ。
6. 休眠クレート(Tauri シェル `ui/cosmo-browse-ui`、`saba/src`、`net/wasabi`、`ui/wasabi`、`renderer-wasm`)は**削除**(git 履歴に残る)。
7. 命名は**平易なパスに統一**: クレートは `cosmo_engine`/`cosmo_script`/`cosmo_runtime` 等、コズミック系モジュール別名(orbit_engine/nebula_renderer/stardust_display)は廃止。`docs/cosmic-naming-migration.md` は更新する。
8. マルチプロセス化(ADR-0001)は**最終フェーズ**。描画忠実度が先。

---

## 目標アーキテクチャ

```
saba/                              (ワークスペース)
├── cosmo_engine/                  ← cosmo_core_legacy を改名; std 化; エンジン本体
│   └── src/
│       ├── dom/       アリーナ Document(NodeId)、木操作、属性、タグ intern
│       ├── html/      トークナイザ、ツリービルダ、実体参照表(生成)
│       ├── css/       トークナイザ、パーサ、CSSOM、at-rule、MediaContext 評価
│       ├── style/     カスケード、プロパティレジストリ、ComputedStyle、
│       │              セレクタマッチング、UA スタイルシート(ua.css)
│       ├── layout/    ボックスツリー、block/inline/flex/grid/float/table/positioned
│       ├── text/      FontMetricsProvider trait、行分割、計測キャッシュ
│       ├── display/   自己記述型ディスプレイリスト(旧 display_item.rs)
│       └── page.rs    ページ単位パイプライン(HttpResponse 依存を除去)
├── cosmo_script/                  ← 新規: boa_engine + DOM バインディング + イベントループ
├── cosmo_runtime/                 ← cosmo_app_legacy を統合改名; loader/session/security/download/レイアウト駆動
├── adapter_native/ adapter_cli/   (役割不変; import 先を新名に)
├── renderer_native/               プラットフォーム層: winit/softbuffer/tiny-skia;
│                                  FontMetricsProvider を fontdue(+ttf-parser)で実装
└── testdata/                      reftests/, sites/(凍結フィクスチャ), html5lib-tests/
削除: saba/src, net/wasabi, ui/wasabi, renderer-wasm, ui/cosmo-browse-ui,
      cosmo_core(シム), cosmo_app_legacy(改名で消滅), scripts_tmp_bench.sh
```

- エンジンは単一クレート+モジュール分割(コンパイル時間が問題化するまでサブクレート化しない)。
- `cosmo_script` のみが Boa に依存。エンジンは JS ランタイムなしでテスト可能に保つ。
- `@media` 用の `MediaContext { viewport_w/h, device_pixel_ratio, prefers_color_scheme, media_type }` は `cosmo_runtime/src/layout/mod.rs` で組み立て(FrameRect は既に `LayoutView::new_with_viewport` へ届いている)、CSS 評価にも渡す。at-rule は無条件で CSSOM に保持し、スタイル解決時に `Stylesheet::effective_rules(&MediaContext)` でフィルタ(リサイズ/ダークモードは再パースなしで再フィルタ)。

## 主要設計判断

**D1. DOM を Rc<RefCell<Node>> → アリーナ + NodeId(u32) へ移行**(Phase 0.8 推奨、遅延時は Phase 3.0 必須)。理由: Boa の GC オブジェクトに Copy な NodeId を持たせるのが唯一安全(Rc をクロージャに捕獲すると回収不能サイクル)/ インクリメンタルレイアウトの dirty bit 置き場 / `Node PartialEq` が kind 比較という既存の罠を根治 / 64MiB スタックの深い再帰 Drop 回避も不要に。`Document { nodes: Vec<NodeSlot>, free: Vec<u32> }`、アクセスは既存 API 名を鏡写しにした Document メソッド経由で呼び出し側変更を最小化(~60–80 箇所)。

**D2. ComputedStyle はプロパティマップでなくグループ化サブ構造体**: `inherited: Rc<InheritedStyle>`(色/フォント系; 継承が安価、Phase 4 のスタイル共有の布石)+ `box_`/`background`/`position`/`flex_grid`/`effects` を `Option<Box<...>>` で。プロパティ解析は `cascading_style` の巨大 match(layout_object.rs:3421)から**プロパティレジストリ**(`style/properties/*.rs`、parse+apply 関数を静的表に登録)へ抽出。`@supports` と Phase 3 の `style.setProperty` もこの表を使う。

**D3. ディスプレイリストを自己記述型コマンドへ**(現状は Rect に ComputedStyle を添付し painter が解釈): `FillRect/Border/FillGradient/BoxShadow/TextRun/Image/PushClip/PopClip/PushTransform/PopTransform`(Phase 4 で ScrollFrame)。`display/builder.rs` がレイアウトツリーを歩いて発行。painter.rs / adapter の SceneItem はほぼ 1:1 のインタプリタに簡素化。IPC スキーマは Phase 5 まで触らない。

**D4. テキスト境界**: エンジンは `FontMetricsProvider` trait(measure/char_advance/register_web_font)越しにメトリクスを得る。行分割(UAX #14)・行ボックス構築はエンジン自作。bidi・複雑シェイピング(rustybuzz)は Phase 5 以降のストレッチ(それまでアラビア語等は崩れる — 許容)。テスト用に現行 `char_advance_16` 表を再現する `FixedMetricsProvider` を同梱し既存 148 テストの期待値を維持。

**D5. Boa↔DOM ブリッジ**: DOM は Boa GC の外(`Rc<RefCell<Document>>` を ScriptHost が所有し `NativeFunction::from_copy_closure_with_captures` に捕獲)。ラッパー JsObject は NodeId を保持、`wrapper_cache: HashMap<NodeId, JsObject>` で `el === el` を保証(ナビゲーションでクリア)。**イベントリスナは JS 側の ListenerRegistry に保持**(DOM 側は has-listeners bit のみ)→ GC サイクルリスク根絶。Boa の job queue を既存のマクロタスク/マイクロタスク/タイマーループ(runtime.rs:215 の骨格は概念維持、実装は `cosmo_script/src/event_loop.rs` へ移植、fuel/反復ガード継続)に接続。

---

## フェーズ計画

工数単位 S = 集中エージェントセッション。各フェーズは reftest+単体テスト green と受け入れサイトのヘッドレススクショ確認で完了。

### Phase 0 — 基盤整備・脱レガシー(8–12 S)

新機能なし。std 化・整理・実フォントメトリクス・テスト基盤。

| # | 作業 | 対象 |
|---|---|---|
| 0.1 | 死クレート削除(saba/src, net/wasabi, ui/wasabi, renderer-wasm, ui/cosmo-browse-ui)、workspace members 整理 | `saba/Cargo.toml` |
| 0.2 | 脱 no_std: `#![no_std]` 除去、`alloc::`→`std::`(~111 箇所)/`core::`→`std::`(~21)、f64 自作数学を std に、`url.rs` 削除、`Page::receive_response(HttpResponse)` → `Page::load_html(html, final_url)` に変えて `http.rs` 削除(呼び出し元: `cosmo_app_legacy/src/layout/mod.rs`) | `cosmo_core_legacy/src/{lib,page,http,url,utils}.rs` |
| 0.3 | 改名: `cosmo_core_legacy`→`cosmo_engine`、シム `cosmo_core` 削除、`cosmo_app_legacy`→`cosmo_runtime`(ラッパー解消)。コズミック別名廃止。CI に `rg` で `saba_|_legacy` 参照禁止 lint。ADR-0003 起草、`docs/cosmic-naming-migration.md` 更新 | ワークスペース全体 |
| 0.4 | 要素モデル開放: `Element { tag: String(intern), known: Option<KnownTag> }`、未知タグを span 化しない。**UA スタイルシート `style/ua.css`**(自前 CSS エンジンで起動時パース)に ~90 要素の既定 display/margin を移す。nav/article/footer/aside/figure = block、video/canvas/svg = プレースホルダボックス | `dom/node.rs`(enum は :240)、`html/parser.rs`(46 match 箇所)、`layout_object.rs`(16 箇所) |
| 0.5 | `layout_object.rs`(4209 行)を**純粋コード移動**で分割: カスケード+プロパティ解析→`style/`、テキスト計測→`text/legacy_metrics.rs`、ペイント発行→`display/builder.rs`、サイズ/位置は `layout/` に残す。`layout_view.rs` のテスト(約半分)も `layout/tests/` へ。移動ごとにテスト green | `renderer/layout/*` |
| 0.6 | `FontMetricsProvider` trait 導入(D4)。`FixedMetricsProvider`(現行数値再現)でテスト維持、`renderer_native` は fontdue 実装。provider を `LayoutView::new_with_viewport`→adapter→レイアウト駆動に貫通。GUI を実メトリクスに切替え、スクショ基準を**専用の1コミット**で再ベースライン | `text/provider.rs`(新)、`renderer_native/src/text_render.rs`、`adapter_native/src/lib.rs` |
| 0.7 | テスト基盤: `scripts/run_layout_reftests.py`(既存ヘッドレススクショで `testdata/reftests/` vs 基準 PNG、許容差は `docs/layout-regression-policy.md` 準拠)。サイトフィクスチャ凍結 `testdata/sites/{hn,abehiroshi,wikipedia,mdn}/` + ローカル配信スクリプト。html5lib-tests をベンダしデータ駆動ランナー(`cosmo_engine/tests/html5lib.rs`、expected-fail リストをコミット) | `scripts/`, `testdata/` |
| 0.8 | **DOM アリーナ移行(D1)**。最リスク項目。遅延時は Phase 3.0 へ(Phase 1–2 はアクセサ層のおかげで非ブロック) | `dom/*`, `html/parser.rs`, `layout_view.rs`, `page.rs` |

受け入れ: 全既存テスト green(新名で)/ HN+abehiroshi スクショ一致(0.6 で1回だけ再ベースライン可)/ html5lib 通過数のベースライン記録 / `rg no_std` ゼロ / layout_object.rs ≤ ~800 行。

### Phase 1 — CSS エンジン近代化(10–14 S)

目標: Wikipedia 級静的 CSS。依存: Phase 0.1–0.7。

| # | 作業 |
|---|---|
| 1.1 | at-rule パース基盤(全捨てをやめる): `@media`(条件 AST)、`@import`(loader へ URL 供給)、`@font-face`(記述子)、`@supports`(プロパティレジストリで評価)、`@keyframes`(Phase 4 用に保存) — `css/cssom.rs` |
| 1.2 | `MediaContext` + 評価器: width/min/max-width/orientation/prefers-color-scheme/and/or/not。リサイズ→再フィルタ+reflow。ダークモードは winit テーマ→adapter→MediaContext |
| 1.3 | プロパティレジストリ(D2)+不足プロパティ: box-sizing、min/max-width/height、font-weight(数値、provider 連動)、font-style、border-style+全辺別 longhand+border shorthand 修正、visibility、overflow-x/y、white-space(pre/pre-wrap)、vertical-align 基本、list-style(-type)、calc()(四則、px/%/em/rem)、単位 rem/vh/vw/vmin/vmax/ch/ex、rgb()/rgba()/hsl()/hsla()+named color 全表+currentColor、inherit/initial/unset。ComputedStyle のサブ構造体化もここで |
| 1.4 | `@font-face` + Web フォント: loader でフォント取得(CSS 取得路+予算を再利用)。TTF/OTF 直、WOFF1 は flate2。**WOFF2 はクレート評価が必要(woff2 系 crate vs brotli+自前コンテナ再構成)— まず TTF/OTF/WOFF1 で出荷し WOFF2 は追補で可**。`register_web_font` + family/weight/style マッチング + フォールバックチェーン |
| 1.5 | 動的擬似クラス: `:hover/:focus/:active/:focus-within` の Never 毒化解除。マウス移動→ヒットテスト(既存)→hover チェーン差分→再スタイル(当面は全文書再解決をフレームレートに間引き)。`:visited` は非対応のまま(プライバシー+簡素) |
| 1.6 | ディスプレイリスト刷新(D3)+ `linear-gradient`/`radial-gradient`(tiny-skia のグラデーションシェーダ)。painter.rs / `replay_paint_commands`(adapter_native/src/lib.rs:1378)簡素化 |

受け入れ: 凍結 **Wikipedia 記事フィクスチャ**が 1280×1024 で読める(見出しウェイト、infobox が本文と重ならない、media query の選択が正しい)/ ダークモードテストページが反転 / Web フォントデモが描画(取得失敗時はフォールバック)/ プロパティ単体テスト +40 / グラデーション・border-style reftest / Phase 0 スイート無回帰。

### Phase 2 — レイアウト完成 + HTML パーサ強化(14–20 S; 最大フェーズ)

目標: MDN 級レイアウト。依存: Phase 1(box-sizing/min-max が flex/grid の前提)。

| # | 作業 |
|---|---|
| 2.1 | **完全 flexbox**(CSS Flexbox L1 §9): `layout/flex.rs` — basis/grow/shrink 解決、min-content クランプ(provider 計測)、wrap+ライン交差軸サイズ、justify-content、align-items/self/content、order、gap |
| 2.2 | **grid v2**: `layout/grid.rs` — px/%/fr/auto/minmax()/repeat() のトラックサイズ、grid-template-areas、線ベース配置+スパン、auto-placement(sparse)、gap。subgrid/masonry は対象外 |
| 2.3 | **float + clear + BFC**: 行ボックスに対する float 配置、clear、BFC 成立条件(root/overflow≠visible/flex/grid item)、float 回り込みの行短縮。**古典レイアウト最難関 — reftest 先行で進める** |
| 2.4 | positioned 正規化: 包含ブロック解決(最近傍 positioned 祖先の padding-box、fixed は ICB)。fixed/absolute のレイアウト後 fixup パス群を in-pass 解決に置換。z-index/opacity/transform の stacking context 整理 |
| 2.5 | **インラインレイアウト書き直し**: 本物のインラインフォーマッティングコンテキスト — 行ボックス、inline-block 参加、provider の ascent/descent によるベースライン、UAX #14 行分割(unicode-linebreak)、書記素(unicode-segmentation)、overflow-wrap/word-break 基本、ellipsis。**回帰面積最大 — 1マイルストーンだけ新旧を LayoutView フラグで A/B し、reftest 比較後に旧経路削除** |
| 2.6 | HTML パーサ: トークナイザに Comment/DOCTYPE トークン、RCDATA/RAWTEXT/script-data 完全化、CDATA、**WHATWG entities.json から生成した全実体参照表**、数値参照(Windows-1252 リマップ含む)。ツリービルダに InTable 系 7 モード+foster parenting、active formatting elements+**adoption agency 本実装**(html5lib-tests をオラクルに; 工数超過時は文書化した近似にフォールバック可)、DOCTYPE→quirks フラグ(保存のみ、挙動は最小)、foreign content(SVG/MathML)を破壊せず DOM 化(描画はプレースホルダ; inline SVG 描画はストレッチ)。DOM に Comment/DocumentFragment 種別追加 |
| 2.7 | iframe/video/canvas を UA スタイル付きプレースホルダボックスとして描画(埋め込みは未対応) |

受け入れ: **MDN 記事フィクスチャ**が重なりなく描画(grid ページシェル、サイドバー、コード欄、表)/ Wikipedia の float サムネイル+キャプション正常 / html5lib tree-construction 通過率 ≥ 85%(template/scripting 除く、expected-fail 単調減少)/ flexbox reftest ~30 + grid ~20 + float ~15 / WPT flexbox サブセット(reftest 化)≥ 80% / 既存スイート無回帰。

### Phase 3 — JS 統合: Boa + DOM バインディング(12–16 S)

目標: 軽 JS サイトが動く。玩具インタプリタ削除。依存: アリーナ DOM(0.8 or 3.0)、2.6(innerHTML 用フラグメントパース)。

| # | 作業 |
|---|---|
| 3.0 | アリーナ移行(0.8 から滑った場合はここで必須) |
| 3.1 | `cosmo_script` クレート: boa_engine を1バージョンに固定、Page ごとに Context、`event_loop.rs`(Boa job=マイクロタスクを既存マクロタスク/タイマーキューへ統合、fuel/反復ガード移植)。その後 `renderer/js/`(玩具、~2.5k 行+37 テスト)を**削除**し cosmo_script テストで置換 |
| 3.2 | DOM バインディング(D5): wrapper cache、Node→Element→HTMLElement/Document/Text/Window プロトタイプ。API: getElementById、**querySelector(All) はエンジンのセレクタマッチャを DOM に対して再利用**(0.5 の style/ 抽出でレイアウト非依存になっている前提; 依存が残っていれば剥がす)、getElementsBy*、createElement/createTextNode、appendChild/insertBefore/removeChild/replaceChild/remove、cloneNode、親子兄弟アクセサ、属性 get/set/has/remove、id/className/classList、textContent、**innerHTML get/set(フラグメントパースモード追加)**、style(setProperty/getPropertyValue+camelCase、プロパティレジストリ経由でインライン style へ) |
| 3.3 | イベント: Event/MouseEvent/KeyboardEvent、capture→target→bubble、preventDefault/stopPropagation、ListenerRegistry。adapter の入力(click/input/keydown/submit)とライフサイクル(DOMContentLoaded/load)を dispatch に接続。デフォルトアクション(リンク遷移/フォーム送信)は prevent されない限り実行 |
| 3.4 | プラットフォーム API: setTimeout/setInterval/clear*、queueMicrotask+Promise(Boa ネイティブ、3.1 で接続)、console.*→既存ログ、location、document.cookie(security.rs のポリシー経由)、navigator。**fetch + XMLHttpRequest**: cosmo_runtime の loader へワーカスレッド委譲、完了をマクロタスクで通知(CORS は既存 security.rs 経路で強制) |
| 3.5 | パイプライン: `MAX_SCRIPT_BYTES` 撤廃(cosmo_runtime/layout/mod.rs:131)。実行順は当面「パース完了後に文書順」(deviation として文書化)、defer≈同等、async は後。**DOM 変異→再レイアウト**: Document に変異世代カウンタ、タスク drain 後 dirty なら style/layout/display 再構築(このフェーズは全再構築で可)。スクリプト例外は隔離(ログしてページ生存) |

受け入れ: vanilla-JS **TodoMVC** フィクスチャが完全動作(クリック/Enter で追加・トグル・削除)/ fetch→リスト描画のテストページ動作 / MDN・Wikipedia を JS 有効で表示しても Phase 2 描画から劣化・クラッシュなし / アコーディオン/タブのデモ動作 / リークチェック: TodoMVC を 100 回ナビゲートして wrapper cache とリスナ登録数がベースラインへ戻る / Wikipedia で `querySelectorAll('div')` < 10ms。

### Phase 4 — パフォーマンス + 動的化(8–12 S)

- 4.1 **インクリメンタル restyle/relayout**: アリーナノードに style/layout dirty bit。hover/class/インライン style 変更は影響サブツリーのみ再解決(無効化は子孫単位の粗さで可)。CI に「インクリメンタル vs フル比較」デバッグアサーションモード。
- 4.2 セレクタ/スタイル性能: 右端単純セレクタ索引(id/class/tag ハッシュ)、祖先ブルームフィルタ、`Rc<InheritedStyle>` 共有。Phase 1–2 の受け入れページが遅ければ前倒し可。
- 4.3 ペイント/スクロール: `diff_scene_items` をディスプレイリスト差分+ダメージ矩形へ拡張。ScrollFrame 付き保持型リストでスクロールは平行移動のみ。painter のグリフラン キャッシュ。
- 4.4 transition + `@keyframes` 再生(1.1 で保存済み): 長さ/色/opacity/transform の補間、イベントループのフレームクロック駆動、requestAnimationFrame。
- 4.5 (任意/ストレッチ) wgpu バックエンドを feature フラグで。tiny-skia が既定のまま。

受け入れ: Wikipedia スクロール 60fps(frame < 16ms、ヘッドレス計測スクリプト+criterion)/ MDN で hover 再スタイル < 5ms / transition デモが全画面再描画なしで動く(ダメージ矩形計測)/ インクリメンタル vs フル比較が reftest 全コーパスで一致。

### Phase 5 — マルチプロセス化(ADR-0001 実現)(10–16 S)

- 5.1 `RendererProcessManager` の `sleep` プレースホルダを実レンダラ実行体(engine+script)に置換。IPC スキーマ v1→v2(自己記述ディスプレイコマンド+入力イベント、`docs/ipc/compatibility-policy.md` 準拠)。ディスプレイリストはバイナリフレーミング or 共有メモリ(bincode/postcard — クレート選定はここで)。
- 5.2 プロセス分離: browser = chrome/session/network/security(cosmo_runtime)、renderer = サイトインスタンスごとのエンジン。クラッシュ回復(sad-tab+再読込)。**前提リファクタ: security.rs のプロセスグローバル static 群(cookie/localStorage/TLS例外/キャッシュ)→ 明示的に渡す per-profile SecurityState**(早いフェーズで機会があれば前倒し可)。
- 5.3 Linux サンドボックス: レンダラの直接 fs/net 禁止(リソースは IPC ブローカ経由)→ landlock/seccomp。
- 5.4 サイト分離基礎: レンダラを site(eTLD+1+scheme)でキー、cookie/storage は browser プロセスのみ。

受け入れ: レンダラ `kill -9` → タブにクラッシュ UI+再読込 / サンドボックス内から open()/socket 不可(統合テスト)/ クロスサイト 2 タブが別 PID / ページロード時間がインプロセス比 ≤1.2× / `scripts/check_ipc_compat.py` green。

---

## 検証戦略(全フェーズ共通)

1. **単体テスト**: 既存 148(0.5 の分割と一緒に移動)を常時 green、フェーズごと +30–50。玩具 JS の 37 テストは Phase 3 で cosmo_script テストに置換。
2. **reftest(回帰網の主力)**: `scripts/run_layout_reftests.py` + 既存ヘッドレススクショ(`--screenshot-wh`、`COSMO_DEBUG_DUMP=1`)。基準 PNG+許容差は `docs/layout-regression-policy.md` 準拠。レイアウト機能は必ず reftest 同梱で着地。
3. **データ駆動オラクル**: html5lib-tests(expected-fail リスト単調減少)、WHATWG entities.json、WPT flexbox/grid の抜粋を reftest 化。
4. **凍結サイトフィクスチャ** `testdata/sites/` をローカル配信(file:// は security で遮断されるため http.server 経由): HN+abehiroshi(P0)→Wikipedia(P1)→MDN(P2)→TodoMVC+対話デモ(P3)→同ページの scroll/hover 性能(P4)→クラッシュ/サンドボックス統合(P5)。生サイト確認は手動、CI はフィクスチャ。
5. **性能**: criterion ベンチ(スタイル解決/レイアウト/セレクタ)+ヘッドレスフレームタイム計測。
6. **ドキュメント規律**: ROADMAP.md v4 を本計画で書き直し。ADR-0003(std 化+エンジン統合)、ADR-0004(Boa 採用+バインディングモデル)、ADR-0005(自己記述ディスプレイリスト)。ADR-0001 は Phase 5 まで不変。

## 主要リスクと緩和

| リスク | 緩和 |
|---|---|
| 0.5 モノリス分割の回帰 | 純粋コード移動のみ+移動ごとにテスト。挙動変更禁止 |
| 0.6 実メトリクス切替で全レイアウトが変わる | 専用1コミットに隔離しフィクスチャ再承認 |
| 0.8 アリーナ移行の big-bang | ソース互換アクセサ API で吸収。滑ったら 3.0 へ(1–2 は非ブロック) |
| 2.5 インライン書き直しの回帰 | 1マイルストーンだけ新旧 A/B フラグ、reftest 比較後に旧経路削除 |
| 2.3 float の複雑性 | 厳密に reftest 先行 |
| 2.6 adoption agency | html5lib-tests をオラクルに。工数上限超過なら文書化した近似で確定 |
| Boa の完成度/速度(JIT なし) | 目標は軽 JS サイト。fuel+実時間 watchdog 維持。Boa 型は cosmo_script 内に封じ込め(バージョン固定) |
| バインディング面の爆発 | 受け入れページが要求した API のみ追加(demand-driven)、未知 API 呼び出しをログ |
| WOFF2 デコード | Phase 1 は TTF/OTF/WOFF1 で出荷可、WOFF2 はクレート評価(1 セッション)後に追補 |
| hover 再スタイルの性能 | 当面フレームレート間引き、Phase 4.1 で本解決 |

## 実装セッションの進め方(他モデル向け)

- 各フェーズ内の項目番号順が依存順(0.2→0.3、0.4→0.5、0.5→0.6)。1 セッション 1〜2 項目が目安。
- 着手前に `saba/HANDOFF.md` と本計画を読み、完了時に HANDOFF.md を更新すること。
- ビルド/テスト/スクショの手順は HANDOFF.md §3 が正(`cargo build -p renderer_native` / `cargo test -p cosmo_core_legacy`(改名後は `-p cosmo_engine`)/ ヘッドレススクショ+`COSMO_SESSION_SNAPSHOT_PATH` 分離)。
- コミット単位はフェーズ表の 1 行 ≒ 1〜3 コミット。0.6(メトリクス切替)と 1.6/2.5(経路置換)は必ず単独コミット。
