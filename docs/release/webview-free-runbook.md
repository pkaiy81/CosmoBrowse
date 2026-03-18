# WebView-free Release Runbook

## Scope

Epic 8 (`E8-T1`〜`E8-T3`) の運用手順をまとめ、`adapter_native` を正式既定経路にしつつ、障害時は `adapter_tauri` 互換経路へ段階的に戻せるようにする。

## 1. Rollout Controls (`adapter_native` default startup)

`src-tauri` は次の feature flag / 環境変数を参照して、デバイスごとに安定した rollout bucket を計算する。

| Variable | Purpose | Default |
|---|---|---|
| `COSMO_RELEASE_CHANNEL` | rollout の既定割合を決める (`dev`/`nightly`/`beta`/`stable`) | `stable` |
| `COSMO_NATIVE_DEFAULT_ROLLOUT_PERCENT` | `adapter_native` を既定化する割合 (0-100) | `stable=10`, `beta=50`, `nightly/dev=100` |
| `COSMO_FORCE_TRANSPORT` | 運用者が `adapter_native` / `adapter_tauri` を強制 | unset |
| `COSMO_DISABLE_ADAPTER_TAURI_FALLBACK` | `1` のとき互換経路への自動フォールバックを止める | unset |
| `COSMO_ROLLOUT_DEVICE_ID` | bucket 計算を固定するデバイス識別子 | hostname / username fallback |

### Rollout sequence

1. `nightly/dev` で 100% `adapter_native` を有効化する。
2. `beta` で 50% に広げ、nightly artifact の `ga-gate-report.json` を監視する。
3. `stable` は 10% から開始し、毎回 `COSMO_NATIVE_DEFAULT_ROLLOUT_PERCENT` を段階的に引き上げる。
4. 3 連続の nightly GA pass を満たすまで `release-gate` workflow が release をブロックする。

## 2. `adapter_tauri` fallback conditions

フロントエンドは次の transport-level failure のときだけ `adapter_tauri` へ落とす。HTTP/TLS/CORS 等のコンテンツ失敗ではフォールバックしない。

- `dispatch_ipc` command が見つからない / 呼び出し不能。
- IPC envelope version が一致しない。
- `dispatch_ipc` に対する引数不整合で Tauri invoke 自体が失敗する。
- 運用者が `COSMO_FORCE_TRANSPORT=adapter_tauri` を設定した。

### Why content failures do not trigger fallback

`adapter_native` と `adapter_tauri` は同じ `cosmo_runtime` / `NativeAdapter` を共有するため、コンテンツ互換やネットワークエラーを transport 差し替えで隠すと原因切り分けが難しくなる。フォールバック対象は「command transport 破損」のみに限定する。

## 3. Rollback procedure

### Session-level rollback

1. `dispatch_ipc` 起動失敗を UI が検知したら、自動で `adapter_tauri` に切り替える。
2. `localStorage` に transport override を保存し、同一端末で再起動しても互換経路を維持する。
3. 直後に `ga-gate-report.json` とアプリログを artifact 保存する。

### Channel-level rollback

1. まず `COSMO_FORCE_TRANSPORT=adapter_tauri` を対象チャネルへ適用する。
2. 必要なら `COSMO_NATIVE_DEFAULT_ROLLOUT_PERCENT=0` を設定し、段階 rollout を完全停止する。
3. `smoke-nightly` を再実行し、`ga-gate-report.json` が回復するまで release を停止する。
4. 修正後、`stable` は 10% から再開し、streak を作り直す。

## 4. Legacy command usage reduction

直接 `invoke()` される旧 command は `scripts/collect_legacy_command_usage.py` で静的集計し、nightly / PR artifact に `legacy-command-usage.json` を出力する。

- 現在の fallback 必須 command: `open_url`, `activate_link`, `get_page_view`, `set_viewport`, `reload`, `back`, `forward`, `get_navigation_state`, `new_tab`, `switch_tab`, `close_tab`, `list_tabs`, `search`
- 削減済みの旧 command: `get_metrics`, `get_latest_crash_report`
- これらの診断系 API は `dispatch_ipc` 経路だけを正式サポートとする。

## 5. GA thresholds

`evaluate_release_gate.py` は次の値を mandatory gate として評価する。

| KPI | Threshold |
|---|---|
| Success rate | `>= 99%` |
| Crash rate | `<= 0.5%` |
| Display time | `<= 1500 ms` |
| Layout regression summary | `pass == true` |

`smoke-nightly` は `kpi_summary.json` / `layout_regression_summary.json` / `legacy-command-usage.json` から `ga-gate-report.json` を生成する。

## 6. Consecutive-pass release blocking rule

- `release-gate` workflow は最新 3 件の `ga-gate-nightly` artifact を取得する。
- `scripts/check_release_streak.py` が `consecutive_pass_streak >= 3` を満たさない限り失敗終了する。
- したがって、GA しきい値を連続達成するまで release は自動的に publish されない。

## 7. Operational checklist

### Before raising rollout percentage

- `smoke-nightly` の `ga-gate-report.json` が pass。
- `legacy-command-usage.json` で新しい direct command が増えていない。
- 障害端末の rollback override 件数が増えていない。

### Before GA declaration

- `release-gate` が連続 3 pass で解除されている。
- stable channel が `adapter_native` 既定起動のまま運用されている。
- `adapter_tauri` は rollback 専用の最小互換面に限定されている。
