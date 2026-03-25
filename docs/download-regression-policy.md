# Download Regression Policy (DL-T4)

最終更新: 2026-03-24

## 1. 目的

`E6-T3` の完了条件として、ダウンロード機能の回帰検証を **PR と nightly で再現可能な形**で固定する。

- 対象: pause / resume / retry / checksum integrity
- 実行スクリプト: `scripts/run_download_regression.py`
- fixture: `scripts/download_fixture_server.py`

## 2. 実行モード

### 2.1 PR（軽量）

PR ではナビゲーション中心の smoke を優先し、ダウンロード回帰は placeholder を残す。

- 生成物: `smoke-artifacts/pr/download_regression_summary.json`
- 内容: `"pass": true`, `"mode": "pr_placeholder"`
- 目的: GA gate schema の必須項目を欠落させない（重い転送ケースは nightly で実施）。

### 2.2 Nightly（本検証）

nightly では実 fixture を起動して検証する。

```bash
python3 scripts/run_download_regression.py \
  --artifacts-dir smoke-artifacts/nightly \
  --fixture-size-mib 64 \
  --timeout-sec 240
```

## 3. 測定仕様（固定）

## 3.1 ケース

1. `pause_resume_checksum`
   - `enqueue_download` → 進捗検知 → `pause_download` → `resume_download`。
   - 最終状態が `completed`。
   - `supports_resume == true`。
   - 出力ファイル SHA-256 が fixture の期待値と一致。
2. `retry_after_cancel`
   - 1 回目を `cancel_download` で中断。
   - 2 回目を再 enqueue して `completed` になること。

## 3.2 失敗分類

- `build_failed`: `native_ipc_cli` の build 不可。
- `timeout`: 指定秒数以内に terminal state へ遷移しない。
- `state_mismatch`: `paused/completed/cancelled` の期待状態を満たさない。
- `checksum_mismatch`: 完了ファイルの SHA-256 が fixture 期待値と不一致。

## 3.3 判定

- `download_regression_summary.json.pass == true` を合格条件とする。
- `scripts/evaluate_release_gate.py` の `checks[name=download_regression].passed == true` を mandatory gate に含める。

## 4. 仕様準拠メモ

- Range resume 検証:
  - RFC 9110 Section 14 (Range Requests)
  - RFC 9110 Section 15.3.7 (206 Partial Content)
- リトライ方針:
  - RFC 9110 Section 9.2.2（GET の idempotent retry 許容）
- validator 不一致時の部分再利用禁止:
  - RFC 9111 Section 4.3 (Validation)

本ポリシーは「測定方法の固定」が目的であり、プロトコル実装の normative spec そのものを置き換えるものではない。
