# Architecture Diagrams (Source of Truth)

このディレクトリの図は `docs/architecture/mermaid/*.mmd` を正本（source of truth）とします。

## 更新責務

- 描画（rendering）・ネットワーク（network）・スクリプト（JavaScript runtime）・IPC（process boundary）のいずれかを変更する PR では、対応する図の更新を **必須** とします。
- 図を更新しない場合は、PR で「影響なし」の理由を明記してください。
- 図変更後は、対応する説明ドキュメント（`*.md`）にも差分要約を反映してください。
- 段階実装中は、代表シーケンス図（`integrated-sequence.mmd`）を常に最新化してください。

## 図と責務の対応

- Runtime Topology（IPC / process boundary）
  - 正本: `mermaid/runtime-topology.mmd`
  - 説明: `runtime-topology.md`
- Render Pipeline（描画）
  - 正本: `mermaid/render-pipeline.mmd`
  - 説明: `render-pipeline.md`
- JS Event Loop（スクリプト実行）
  - 正本: `mermaid/js-event-loop.mmd`
  - 説明: `js-event-loop.md`
- Security Boundary（ネットワーク/セキュリティ境界）
  - 正本: `mermaid/security-boundary.mmd`
  - 説明: `security-boundary.md`
- Integrated Browser Sequence（URL入力→取得→解析→描画→イベント→再描画）
  - 正本: `mermaid/integrated-sequence.mmd`
  - 説明: `integrated-sequence.md`

## レビュー必須項目（PR）

以下に変更がある PR は、対応図更新チェックを必須化します。

- [ ] 描画変更: Render Pipeline 図を更新した（または影響なし理由を記載）
- [ ] ネットワーク変更: Security Boundary 図を更新した（または影響なし理由を記載）
- [ ] スクリプト変更: JS Event Loop 図を更新した（または影響なし理由を記載）
- [ ] IPC変更: Runtime Topology 図を更新した（または影響なし理由を記載）
- [ ] 段階実装変更: Integrated Browser Sequence 図を更新した
- [ ] ADR-0001 のレビュー観点（責務/障害時方針/IPC境界/依存方向）を確認した


## ADR（設計決定記録）

- ADR-0001: Process Model / Failure Policy / IPC Boundary / Dependency Direction
  - `adr-0001-process-model-and-ipc-boundary.md`
