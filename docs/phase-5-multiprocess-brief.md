# 専用セッション指示書: Phase 5 「マルチプロセス化」（ADR-0001 実現）

> 独立セッション用。着手前に `saba/HANDOFF.md` 全体と本書を読むこと。**ロードマップ最終フェーズ・最大規模**。描画忠実度(Phase 1–4)が概ね整ってから着手するのが本来の順序。

## 背景 / 現在地

- 現状は**インプロセス**: `adapter_native`(`Mutex<BrowserApp>`)+ `renderer_native`(winit GUI, `AppBridge` が `LivePage` を保持)。GUI とエンジンは同一プロセス。
- `adapter_native` に IPC の骨格(`native_ipc_cli` バイナリ、`ProcessHost` trait、`RendererProcessManager` 相当)がある。プロセス分離の実体(実レンダラ実行体)は未接続/プレースホルダ。
- `cosmo_runtime/src/security.rs` は**プロセスグローバル static**(cookie/localStorage/TLS 例外/キャッシュ、`OnceLock<Mutex<...>>`)を多用。
- Boa `Context` は `!Send`(D5 で per-page 状態は host に載ったが host 自体はスレッド固定)。

## ゴール（ADR-0001）

browser プロセス(chrome/session/network/security)と renderer プロセス(サイトインスタンスごとの engine + script)に分離。IPC で自己記述ディスプレイコマンド + 入力イベントをやり取り。クラッシュ回復・サンドボックス・サイト分離。

## 難所

1. **前提リファクタ**: security.rs のプロセスグローバル static 群 → 明示的に渡す **per-profile `SecurityState`**。これは早いフェーズで前倒し可(独立して価値あり)。
2. ディスプレイリストの**バイナリフレーミング/共有メモリ**(bincode/postcard 等、クレート選定)。IPC スキーマ v1→v2。
3. renderer プロセスは Boa/DOM を持つ(!Send だが各プロセス単一スレッドなら可)。browser↔renderer の入力/出力を IPC で。
4. **Linux サンドボックス**(landlock/seccomp)、リソースは IPC ブローカ経由。
5. サイト分離(eTLD+1+scheme でキー、cookie/storage は browser プロセスのみ)。

## 推奨アプローチ（段階的）

1. **security.rs の脱プロセスグローバル**(前倒し可): static 群を `SecurityState` にまとめ、per-profile で明示的に渡す。単体で landing 可能。
2. **IPC スキーマ v2**: 自己記述ディスプレイコマンド + 入力イベント。`docs/ipc/compatibility-policy.md`(あれば)準拠。シリアライズ(postcard/bincode)。
3. **RendererProcessManager 実体化**: `sleep` プレースホルダ → 実レンダラ実行体(engine+script)。browser = chrome/session/network/security、renderer = サイトインスタンス。
4. **クラッシュ回復**: renderer `kill -9` → sad-tab UI + 再読込。
5. **サンドボックス**: renderer の直接 fs/net 禁止 → landlock/seccomp。
6. **サイト分離**: renderer を site でキー、cross-site 2 タブが別 PID。

## 検証

- renderer `kill -9` → クラッシュ UI + 再読込。
- サンドボックス内から `open()`/`socket` 不可(統合テスト)。
- cross-site 2 タブが別 PID。ページロード時間がインプロセス比 ≤1.2×。
- `scripts/check_ipc_compat.py`(あれば)green。既存 GUI/スクショ経路が壊れないこと。

## 撤退ライン

フルのプロセス分離が重い場合、**(1) security.rs の per-profile SecurityState 化** と **(2) IPC スキーマ v2(自己記述ディスプレイコマンド)** の2つだけでも独立して landing(将来のプロセス分離の前提が整う)。実プロセス分離・サンドボックスは後続。

## 関連ファイル

- `saba/adapter_native/src/lib.rs`(`ProcessHost`/IPC)、`adapter_native/src/bin/native_ipc_cli.rs`。
- `saba/cosmo_runtime/src/security.rs`(プロセスグローバル static → SecurityState)。
- `saba/cosmo_runtime/src/session.rs`、`saba/renderer_native/`。
- `docs/adr/`（ADR-0001 / 起草予定の ADR）、`docs/ipc/`。
