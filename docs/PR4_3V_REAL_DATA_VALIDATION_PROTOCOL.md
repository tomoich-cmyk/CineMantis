# PR4-3V: 実データの read-only 検証 — 手順（設計のみ）

状態: **設計だけ。実行していない。** production DB には触れていない。holdout は封印のまま。FINAL は NOT_RUN。policy freeze は未定義。
前提: PR4-3 が commit 済み（`67e6f41`）、bridge は `READY`（SHA-256 `0f31ea96…`）。

## 目的

凍結した bridge と、DB の信頼できる GT（PR4-2 の allowlist）が、**正式 closeout の 44 件に本当に収束するか**を、実データで最初に確かめる。次に、`diagnostic_gt` が PASS するか BLOCKED になるかを見る。BLOCKED は想定される結果の 1 つで、失敗ではない（原因を件数だけで特定するのが目的）。
診断 precision の値の評価は、この gate の目的ではない（PASS したときに値が出るが、policy freeze の前の参考値で、昇格には使えない）。

## 守ること

- production DB に書かない。production DB を**書き込み可能な接続で開かない**（開くと WAL の checkpoint で DB 本体が変わりうる）。
- **アプリを終了してから**行う。アプリが動いている間の DB は、WAL に未反映の変更があり、read-only 接続からは見えないことがある（過去に実証済み）。「変更なし」と判断するのに、read-only 接続だけに頼らない。
- holdout の中身（ラベル・verdict・候補）を読まない・出力しない。出力は**件数だけ**（GT の identity・作品 ID・題名を出さない）。
- 出力はリポジトリの外（`CineMantis-audit\pr4\real_data\`）。リポジトリに入れるのは、手順と、件数の要約（別途承認）だけ。

## 手順

1. **前提の確認**: アプリが終了していること。`TMDB` など外部への通信は不要（実行しない）。
2. **現物の記録**: production の `cinemantis.db` / `-wal` / `-shm` のサイズ・更新時刻・SHA-256 を記録する（読むだけ）。
3. **生ファイルのコピー**: 3 つを、変更せずに作業用ディレクトリへコピーする（コピー元は開かない）。コピーの SHA-256 が元と一致することを確認する。
4. **コピー側で recovery**: コピーした DB を**コピー側だけ**で読み書き可能に開き、WAL を取り込む（production には何も起きない）。
5. **論理スナップショット**: コピー側から immutable なスナップショットを作り、SHA-256 を記録する。runner 側（書き込み可能な接続）の件数と突き合わせ、一致しなければ STOP。
6. **レポートの生成**: スナップショットを `query_only` で開き、`generate_report(DevelopmentAudit, Some(GtConfig::production(bridge_path)))` を実行する。bridge のパスは引数で渡す。
7. **結果の記録**: レポートの JSON を保存し、SHA-256 を記録する。production DB の SHA-256・サイズ・更新時刻が手順 2 から変わっていないことを再確認する。

## runner（実装が必要。承認後）

- `gt_operator.rs` と同じ流儀の、**手動実行専用**（`#[cfg(test)]` + `#[ignore]`、環境変数で名前を指定したときだけ動く）テスト 1 本。製品には出さない。
- 入力: スナップショットのパス、bridge のパス、出力先（環境変数）。DB は `query_only` で開き、書き込みを試みない。
- 出力: `generate_report` の JSON（件数のみ）。標準出力には要約（status・blockers・件数）だけ。
- production DB のパスは runner に渡さない（スナップショットだけを渡す）。

## 結果の読み方

| `diagnostic_gt.status` / blockers | 意味 | 次 |
|---|---|---|
| `PASS`、`gt_universe` が 50 / 44 / 6 | bridge + DB の GT が正式 closeout に収束した | precision の値は参考値として記録。policy freeze の議論へ |
| `gt_universe_mismatch` | DB 側の信頼できる GT の件数が期待（40）と違う。`gt_universe` / `excluded_gt` / `unresolved_gt_count` の件数で原因を特定 | 原因ごとに別の対応（例: 上書きされた GT = `gt_superseded`、sample が合わない、DB にラベルが無い） |
| `gt_source_conflict` | DB と bridge で、同じ評価単位の canonical GT が違う | 重大。原因の調査（どちらも書き換えない） |
| `evaluation_key_mismatch` / `cohort_boundary_failure` | DB の task と bridge のキーが合わない / bridge の単位が封印された cohort を指す | 重大。調査 |
| `unsupported_label_semantics` / `invalid_auto_output` | 想定外のラベル / AUTO の記録 | 件数で調査 |
| `bridge_*` | bridge の検証失敗 | bridge は凍結済み。SHA とパスを確認 |

## 停止条件（どれかで STOP して報告）

アプリが動いている / production DB の SHA・サイズ・更新時刻が手順の前後で変わった / コピーの SHA が元と違う / スナップショットと runner の件数が一致しない / レポートに GT の identity・作品 ID・題名が含まれている / holdout の内容に触れた疑い。

## この gate がやらないこと

DB への書き込み・書き戻し / 通信 / bridge の変更 / D・F の GT の DB への取り込み / holdout の評価 / FINAL の計算 / policy freeze / 昇格判断 / 診断 precision の値の評価（PASS しても参考値）。

## 承認が必要な点

1. runner の実装（上記の手動専用テスト）。
2. production の DB パスの確認と、アプリを終了した状態での実行（実行はあなたの端末で、あなたが行う手順を含む）。
3. 出力の置き場所（`CineMantis-audit\pr4\real_data\`）。

## 承認済みの改訂（PR4-3V0 の監査コメントによる）

- **production の DB を SQLite として一度も開かない。** 停止中の DB / WAL / SHM を byte copy し、コピー側だけで recovery して作った immutable snapshot だけを runner に渡す。WAL / SHM が最初から無ければ「不存在」を記録し、作らない。
- 実行の順序: ① アプリを完全終了 ② production の DB / WAL / SHM の metadata + SHA を取得 ③ 3 ファイルを byte copy ④ production 側は二度と触らない ⑤ コピー側だけで recovery ⑥ recovered main DB を immutable snapshot 化 ⑦ snapshot の SHA を固定 ⑧ runner を snapshot だけに実行 ⑨ production の metadata + SHA を再取得 ⑩ 前後の完全一致を確認。**1 byte でも変わっていれば、結果が PASS でも STOP。**
- runner の入力は環境変数 6 つだけ（`docs/PR4_3V0_REPORT.md`）。production の DB のパスを受け取る入口は無い。
- 出力は `CineMantis-audit/pr4/real_data/<UTC_RUN_ID>/`（Windows では `CineMantis-audit` 配下の `pr4`、`real_data`、実行 ID の順）。GT の identity（sample_id / task_id / work_id / tmdb_id / 題名 / 候補）は出さない。
- この文書の「runner の実装」は PR4-3V0 で実施した。**production の DB のコピー・snapshot の作成・実データ実行は、runner の監査後の別 gate**（未承認）。
