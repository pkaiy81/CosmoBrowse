## Summary
- 

## Changes
- 

## 更新した図（必須）
- 対象図: 
- 正本ファイル（`docs/architecture/mermaid/*.mmd`）: 
- 変更概要: 

## 仕様コメント（RFC/HTML/DOM/CSS）追記箇所（必須）
- 対象仕様: 
- 追記ファイル/箇所: 
- コメント内容要約: 

## Dependency / Merge Order Check
- [ ] foundation → layout/render → js/security → process/devtools のマージ順を確認した

## Architecture Diagram Review Gate（該当時必須）
- [ ] 描画（rendering）を変更した場合、`render-pipeline` 図を更新した（または影響なし理由を記載した）
- [ ] ネットワーク（network）を変更した場合、`security-boundary` 図を更新した（または影響なし理由を記載した）
- [ ] スクリプト（script runtime）を変更した場合、`js-event-loop` 図を更新した（または影響なし理由を記載した）
- [ ] IPC を変更した場合、`runtime-topology` 図を更新した（または影響なし理由を記載した）
- [ ] 段階実装を進めた場合、`integrated-sequence` 図を最新化した


## ADR-0001 Review Gate（必須）
- [ ] プロセス責務を逸脱していない（Browser / Renderer / Network / 将来 GPU の境界を維持）
- [ ] Renderer 異常終了時の方針を満たす（再起動条件・セッション維持範囲・再試行上限）
- [ ] IPC 境界が `IpcRequest` / `IpcResponse` の DTO 契約のみである（UI 固有型を含めない）
- [ ] 依存方向が `adapter_* -> cosmo_runtime -> cosmo_core` のみである

## Checklist
- [ ] 作業開始時に新規ブランチを作成した（`feature/<track>-<scope>`）
- [ ] ブランチ名が命名規約に準拠している
- [ ] 変更内容に対応するテスト/検証を実施した
- [ ] ドキュメントを更新した（必要な場合）
