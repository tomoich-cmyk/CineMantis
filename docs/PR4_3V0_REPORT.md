# PR4-3V0: 実データ検証 runner — 実装報告

状態: **runner の実装・offline テスト・凍結まで。commit していない。** production の DB には一切触れていない（コピー・snapshot 作成・実データ実行は未実施で、runner の監査後の別 gate）。holdout は封印のまま。FINAL は NOT_RUN。
基点: HEAD `67e6f41bcee9746258c3fdb16c2a1d77604cae04`。

## 変更したファイル
| ファイル | 内容 |
|---|---|
| `apps/desktop/src-tauri/src/services/pr4_3v_runner.rs`（新規） | runner 本体（`#[cfg(test)]`・手動専用）と 13 件のテスト |
| `apps/desktop/src-tauri/src/services/mod.rs` | `#[cfg(test)] pub mod pr4_3v_runner;` の 2 行（test ビルドだけ） |
| `docs/PR4_3V_REAL_DATA_VALIDATION_PROTOCOL.md`（新規）、`docs/PR4_3V0_REPORT.md`、`docs/PR4_3V0_FREEZE.txt` | 手順（承認済みの改訂を追記）、この報告、凍結ハッシュ |

製品のコード・`pr4_diagnostics.rs`・migration・DB・matcher / rules / Jev・過去の凍結ファイルは変えていない。

## runner
`gt_operator.rs` と同じ流儀。製品の command / 通常経路には出ない。実行するのは `#[ignore]` のテスト `manual_pr4_3v_validate` 1 本で、`CINEMANTIS_PR4_3V_ACTION=validate` でなければ STOP（環境変数が無いまま実行すると STOP することを確認済み）。

**入力は環境変数 7 つだけ**: `CINEMANTIS_PR4_3V_ACTION` / `_SNAPSHOT_DB` / `_SNAPSHOT_MANIFEST` / `CINEMANTIS_PR4_GT_BRIDGE` / `CINEMANTIS_PR4_3V_OUTPUT_DIR` / `CINEMANTIS_PR4_3V_EXPECTED_HEAD`（**評価対象** = PR4-3 の実装 `67e6f41…`）/ `CINEMANTIS_PR4_3V_RUNNER_HEAD`（**runner 自体**の commit。runner の commit の後の HEAD。評価対象とは別の SHA）。production の DB のパスを受け取る引数・環境変数は無い。

**実行前の確認（どれか 1 つでも違えば STOP・出力は作らない）**: snapshot が存在 / production のアプリデータ置き場（`dev.cinemantis.app`）の中ではない / snapshot の SHA-256 が manifest と一致 / snapshot の隣に `-wal` `-shm` が無い / expected HEAD（評価対象）= `67e6f41…`（凍結値）/ manifest の `source_head` が一致 / manifest の `runner_head` が runner HEAD（40 桁）と一致 / bridge の SHA-256 = `0f31ea96…`（凍結値）/ manifest の版・protocol・各 SHA の形式・実行前の production の記録（DB / WAL / SHM の present / absent と size・mtime・SHA）/ cohort は A1 だけ（A2 などは STOP）。

**開き方**: `SQLITE_OPEN_READ_ONLY` + URI `?mode=ro&immutable=1`、さらに `PRAGMA query_only = ON`（`generate_report` の中でも ON）。書き込み可能な接続を作らない（ソースを検査するテストあり）。

**結果の読み順（固定）**: `diagnostic_gt.status` → `gt_universe.resolved` / `deferred` → `blockers`。1 つでも blocker か件数の不一致があれば `validation = BLOCKED` で、precision は評価しない（レポートに参考値を出さない）。PASS（44 / 6）のときだけ precision を「参考値（昇格には使えない・44 は分母ではない）」として記録する。

**出力**（`<OUTPUT_DIR>\<UTC_RUN_ID>\`）: `snapshot_manifest.json`（検証済みの manifest の写し）/ `diagnostic_report.json` / `PR4_3V_RUN_REPORT.txt`。件数・率・blocker だけ。sample_id・task_id・work_id・tmdb_id・題名・候補を含むキー / 文字列が 1 つでもあれば、書かずに STOP。既存の run ディレクトリには上書きしない。出力先以外へは書かない。

## テスト（合成した snapshot だけ。production の DB・実 bridge・holdout は使わない）
| 実行 | 結果 |
|---|---|
| `cargo test --lib pr4_3v_runner` | 13 passed / 0 failed / 1 ignored（手動 runner） |
| `cargo test --lib`（全件の回帰） | 499 passed / 0 failed / 9 ignored（もともとの 8 + 手動 runner 1） |
| `cargo check --lib` | 通過（このモジュールの警告なし） |

13 件が確かめること: 44 / 6 の収束で PASS（出力は 3 ファイル、読み順、`final_promotion` は NOT_RUN）/ GT universe が 43・45 → BLOCKED（precision を出さない）/ DB と bridge の GT の衝突 → BLOCKED / 環境変数の不足・ACTION 違い → STOP / snapshot SHA の不一致・存在しない・隣に `-wal` / `-shm`・HEAD の不一致（expected・manifest）・bridge の不一致・manifest の不備（protocol、SHA の形式、production の記録の欠落・不正、作成時刻）→ STOP（出力を作らない）/ 凍結 bridge の SHA と違う合成 bridge は本番設定で STOP / production のデータ置き場の中の snapshot → STOP / A1 以外 → STOP / snapshot は read-only で書き込めず、1 byte も変わらず、`-wal` `-shm` も作らない / ソースに書き込み可能な接続・書き込み文・production のパス・通信が無い / レポートに identity が出ない（検査自体も検証）/ 出力先の外に書かない・再実行は上書きしない・不正な run id は拒否 / test ビルドの手動専用 / run id と時刻の形式。

テスト中の誤りを 4 件直した（実装の不具合ではない）: run id の長さの期待値（16 文字）/ 43・45 の fixture で bridge が 43 と宣言していたため先に `bridge_not_ready` になっていた（実際の bridge は 44 / 6 を宣言するので、fixture も常に 44 / 6 にした）/ レポートの定義文「exact_media_type_tmdb_id」「(tmdb_id, media_type)」を identity と誤検出していた（定義の説明文で、identity ではない。検査はキー名と固定文字列に対して行う）/ Windows で追記モードのままでは `set_len` が拒否される。

## 注意
- テストは一時ディレクトリに fixture を作り、削除しない（OS の temp 配下。実害なし）。
- 本物の bridge を使う検証は、実データ実行の gate で行う（この gate では合成 bridge のみ）。
- 実データでの結果（PASS か BLOCKED か）は未確認。

## 次（未承認）
runner の監査 → 別 gate で、production の snapshot 作成（アプリ終了・byte copy・コピー側 recovery・SHA 固定・前後の完全一致確認）と、runner の実行。

## 改訂: 評価対象の HEAD と runner の HEAD を分ける（監査の推奨）

- `evaluation_head` = `67e6f41bcee9746258c3fdb16c2a1d77604cae04`（PR4-3 の実装。凍結値として固定）。
- `runner_head` = この runner を載せた test-only の commit の SHA（別の SHA。実行 gate で env と manifest に渡し、レポートに別々に記録する）。runner の commit は評価対象ではない。
- テスト 13 件のまま（manifest の `runner_head` の不一致・形式不正を検査する分を既存のテストに追加）。
