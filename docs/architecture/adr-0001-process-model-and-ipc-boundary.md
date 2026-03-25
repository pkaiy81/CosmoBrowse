# ADR-0001: Process Model / Failure Policy / IPC Boundary / Dependency Direction

- Status: Accepted
- Date: 2026-03-16
- Deciders: CosmoBrowse maintainers
- Related: `docs/architecture/runtime-topology.md`, `docs/architecture/contracts.md`

## Context

CosmoBrowse は Browser Process を中心に段階的にマルチプロセス化を進めている。現状の設計意図（責務分離、障害時の挙動、IPC 境界、依存方向）が文書間に分散し、レビュー時の判断基準がぶれやすい。

そのため、本 ADR で以下を単一の規約として固定する。

1. プロセス責務（Browser / Renderer / Network、将来 GPU）
2. Renderer 異常終了時の運用方針
3. `IpcRequest` / `IpcResponse` の責務と禁止事項
4. レイヤ依存方向
5. PR レビュー観点

## Decision

### 1) Process responsibility

#### Browser Process
- プロダクト全体のオーケストレーション責務を持つ。
- ナビゲーション開始・履歴・セッション管理・権限/セキュリティ判定を司る。
- Renderer/Network 各 Process への要求を `IpcRequest` で発行し、統合結果を UI に返す。

#### Renderer Process
- DOM 構築、レイアウト、ペイント対象生成、スクリプト実行（許可範囲内）を担当する。
- Browser Process から受け取った入力（HTML/CSS/イベント）を処理し、描画可能な結果を返す。
- 外部ネットワークへの直接アクセスは行わない（必ず Browser 経由）。

#### Network Process
- DNS/TCP/TLS/HTTP を含むリソース取得、接続管理、ネットワーク診断ログを担当する。
- CORS/証明書検証/リダイレクトなどネットワーク境界の判定を担当する。
- DOM/レイアウト/JS 実行には関与しない。

#### （将来）GPU Process
- コンポジット、ラスタライズ等の GPU 依存処理を担当する想定。
- Renderer から分離し、GPU 障害が Browser 全体停止に波及しない構成を目標とする。

### 2) Failure policy (Renderer crash)

Renderer 異常終了を検知した場合、Browser Process は以下を適用する。

- 再起動条件:
  - プロセス終了コード異常、heartbeat timeout、IPC channel 断を異常終了とみなす。
  - 異常終了時は同一タブの Renderer を再生成する。
- セッション維持範囲:
  - Browser 管理データ（履歴、Cookie/Storage の永続領域メタ情報、権限状態）は維持する。
  - Renderer 内の一時状態（進行中 JS 実行、未確定 DOM 変更、in-memory layout cache）は破棄する。
- 再試行回数:
  - 連続異常終了に対して最大 2 回まで自動再起動（初回 + 再試行 2 回 = 最大 3 起動）。
  - 上限超過時は当該タブを「クラッシュ画面」に遷移し、自動再試行を停止する。

### 3) IPC boundary (`IpcRequest` / `IpcResponse`)

- Browser ↔ Renderer 間の契約は `IpcRequest` / `IpcResponse` を唯一の公開境界とする。
- IPC payload はフレームワーク非依存 DTO のみで構成する。
- 禁止事項:
  - UI 固有型（例: Tauri/Wry/WebView などのフレームワーク型）を `IpcRequest` / `IpcResponse` に含めない。
  - 関数ポインタ/クロージャ/参照寿命に依存する型を IPC 契約に持ち込まない。
  - Renderer 内部最適化データ構造をそのまま露出しない。
- 許可事項:
  - 安定化した列挙型・構造体・文字列/数値/配列等のシリアライズ可能データ。
  - バージョニング可能なフィールド追加（後方互換を壊さない拡張）。

### 4) Dependency direction

許可される依存方向を以下に固定する。

- `adapter_* -> cosmo_runtime -> cosmo_core`

制約:
- 逆方向依存（`cosmo_core` から `cosmo_runtime`/`adapter_*` 参照）を禁止する。
- `adapter_*` 同士の直接依存を禁止する。
- UI/OS 固有実装は `adapter_*` に閉じ込める。

### 5) ADR-based review checklist

以下を PR レビュー時の必須観点とする。

- プロセス責務逸脱がないか（Renderer が network を直接触っていないか等）
- Renderer クラッシュ時の再起動条件・セッション維持範囲・再試行上限を壊していないか
- IPC 境界に UI 固有型/非シリアライズ前提型が流入していないか
- 依存方向が `adapter_* -> cosmo_runtime -> cosmo_core` を満たすか

## Consequences

- 利点:
  - 実装判断とレビュー観点が統一され、設計逸脱の早期発見が可能になる。
  - マルチプロセス化の段階実装において互換性のある IPC 契約を維持しやすい。
- トレードオフ:
  - 一部の最適化（内部型の直接受け渡し）が制限される。
  - 障害時の自動再試行に上限を設けるため、回復不能時はユーザ操作が必要になる。

## Follow-ups

- PR テンプレートに ADR-0001 レビュー項目を追加する。
- 必要に応じて runtime-topology 図へ crash-restart policy の注記を追加する。
