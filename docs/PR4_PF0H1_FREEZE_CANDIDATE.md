# PR4-PF0H1 FREEZE CANDIDATE（H2 close ＋ 独立 verify）

判定: **PASS（freeze candidate）。** commit / push / merge は未実施。Human の明示承認待ちで STOP。
実 H2 の ledger / checkpoint / payload / deviation log、production DB は一切読んでいない。main H2 worktree・daily runner・PF0E tool / rules は変更していない。synthetic / fixture のみ。

## 1. 作業の出発点と base
- 出発点: PF0H0/PF0H1 の成果物（branch `claude/pr4-pf0h-455c28`、commit `556a99f`、contract r2）。
- この worktree（`claude/pr4-pf0h1-e60774`）は `556a99f` へ fast-forward のみで取り込んだ（履歴の書き換えなし）。
- 556a99f は main H2 HEAD `69d150d633ad1097319e27545962d989a92aebd9` の直接の子で、`69d150d..HEAD` の差分は PF0H の 6 ファイルだけ（下記 §5）。詳細は `PR4_PF0H1_LATEST_MAIN_COMPAT.md`。

## 2. 最終条件の充足
| # | 条件 | 状態 | 根拠 |
|---|---|---|---|
| 1 | membership parity | 充足 | `the_authoritative_membership_commitment_is_byte_identical_to_the_pf0b_definition`。close の出力・verify の受理値が PF0B `membership_commitment` と一致。(1) PF0B 関数そのもの、(2) 手で組んだ pre-image（domain `pr4-pf0a-membership-1`、key 辞書順）の SHA、(3) 補助値（`unit_set_sha256` 等）は `authoritative:false` で正本と別物、(4) 補助値へのすり替えを verify が拒否。PF0H は PF0B の関数を呼ぶだけで定義を再解釈しない。PF0B の関数本体と domain は commit 3d72255 時点と同一であることを確認済み |
| 2 | latest main 互換 | 充足 | base が 69d150d を含む。再実行結果は §4 |
| 3 | close / verify の分離 | 充足 | `pr4_pf0h_close.rs` と `pr4_pf0h_verify.rs` は別 module。共有は PF0B の関数のみ。ソース走査テスト T45 が相互参照なし・verify が書き込み API を持たないことを固定 |
| 4 | waive 不可の hard STOP | 充足 | uncovered tail `E_UNCOVERED_TAIL` / chain break `E_LEDGER_CHAIN`・`E_CHECKPOINT_CHAIN` / payload 欠落 `E_PAYLOAD_MISSING` / 孤児 payload `E_ORPHAN_PAYLOAD` / unknown tool SHA・rollback `E_TOOL_SHA` / `E_REPORT_MISSING` / membership 不一致 / 未宣言 gap `E_UNDECLARED_GAP` / close 後 checkpoint。いずれも pin による waive 経路なし（waive できるのは pin 済み `extra_run` のみ）。各々に synthetic test あり |
| 5 | close 後 checkpoint | 充足 | no-op checkpoint 1 本でも `E_POSTCLOSE_CHECKPOINT`（`t06_to_t08`） |
| 6 | 10/10 note | 充足 | tool は note 本文を読まない。params で JST 日付 `2026-10-10`・SHA `5f809726…68b68`・disposition `accepted_non_material`・backfill `forbidden`・membership/checkpoint impact `none` を固定値照合するだけ。audit 側 preflight の SHA を `E_EXCEPTION_PREFLIGHT` で必須化。exception は 1 件のみ（2 件・別日・属性違いは `E_PARAMS_INCOMPLETE`）。V6(c) は exception があっても hard |

tool SHA 時系列: checkpoint seq 1–3 は OLD `183e0c73…e3213` のみ、seq ≥ 4 は NEW `36f869f6…18d08` のみ（`tool_sha_is_checked_as_a_timeline_not_a_set`）。

## 3. 今回の変更（この gate で加えたもの）
- `pr4_pf0h_close.rs`: params 検証内の重複した `audit_note_sha256` 条件を 1 つに整理（挙動不変）。
- contract: synthetic test 件数の記述を現状（25 件）に更新。
- 本 report と `PR4_PF0H1_LATEST_MAIN_COMPAT.md` を追加。
- 設計・実装・schema・test の本体は 556a99f のもの（既に最終条件の大半を満たしていたため、新規ロジックは足していない）。

## 4. 検証結果（この worktree、cargo test --lib）
- `pr4_pf0h`: 26 passed / 0 failed / 1 ignored（manual 実行用 close entrypoint）。
- `pr4_`（PF0B・PF0E・PF0H ほか関連回帰）: 137 passed / 0 failed / 5 ignored。
- source scan（T45）と PF0E verify_ledger との判定一致（T46）は上記に含まれ PASS。
- close の再実行は決定的（T01/T42: `generated_at` 以外 byte 一致）、入力 dir は不変（T40/T48）。

## 5. diff
base `69d150d` に対して（working tree 含む）: mod.rs +9、`pr4_pf0h_close.rs`、`pr4_pf0h_verify.rs`、`pr4_pf0h_tests.rs`、contract、schema の 6 ファイルが追加・変更。既存の PF0B / PF0E / daily runner のファイルに差分なし。`git diff --stat 69d150d` の実行結果は最終報告に記載。

## 6. 残リスクと Human 判断事項
1. version 文字列の組立（`pr4-h2-enrollment-rules-2/enrollment_rule_sha256=<rules sha>`）は PF0H 側の選択。PF0B は version を入力として受けるだけなので、監査で「FINAL 時に PF0B へ渡す version はこれ」と確認が必要（commitment は version にも依存する）。
2. 実 H2 に対する close の本番実行は未実施。実行前に audit 側で note ファイルの実在と SHA 再計算、daily schedule 停止（`E_POSTCLOSE_CHECKPOINT` 回避）を行う運用手順が前提。
3. 外部アンカー（auditor_anchors / close anchor）は params への転記が必要。これは実データ依存のため未確認。

## 7. 判定
**PASS（freeze candidate）。** HOLD / STOP 要因なし。次は Human の承認を待って commit → merge。承認前は何も commit しない。
