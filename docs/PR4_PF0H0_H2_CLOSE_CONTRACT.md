# PR4-PF0H0: H2 close と FINAL membership freeze の契約（r2・PF0H1 で実装済み・未実行）

状態: **設計のみ。** 実 H2 の ledger / checkpoint / payload / deviation log は一切読んでいない。production DB・enrollment tool・rules は変更しない。machine output・性能は存在しない。FINAL は NOT_RUN。
機械可読版: `docs/pr4_pf0h0_close_manifest_schema.json`（manifest 3 種 + 事前固定 params の schema）。

## r2 Amendment（監査裁定。**以下は本文より優先する**）
1. **authoritative な membership commitment** は PF0B の `membership_commitment(keys, version)`（domain `pr4-pf0a-membership-1`）をそのまま使う。key = `work:<id>`、version = `pr4-h2-enrollment-rules-2/enrollment_rule_sha256=<rules sha>`（この組立は PF0H1 の実装上の選択。監査で確認してほしい）。`unit_set_sha256` / `unit_input_sha256` は **補助の監査値**（`authoritative:false`）で、`FINAL_MEMBERSHIP_COMMITMENT.json` では `membership_commitment_sha256` が正本。
2. **V6(c)（最後の checkpoint が ledger 全 record をカバー）は hard。waive 不可。**
3. **close 後:** close を実行した run 自身の checkpoint が chain の最後。**close 後の no-op checkpoint も許さない**（後続 checkpoint があれば `E_POSTCLOSE_CHECKPOINT` で STOP）。→ 本文 V6(d) と `postclose_noop_checkpoints`、§6-2 の「許容」記述は廃止。**運用上の注意:** 日次 schedule は close を出した run の直後〜次の定刻より前に停止しないと、次の no-op run が checkpoint を足して close が通らなくなる。停止を close 検知の最初の手順にすること。
4. **deviation subset commitment** は出してよいが **audit-only**（`audit_only:true, promotion_use:"forbidden"`）。alternate precision・sensitivity に基づく promotion 判断は禁止。
5. **deviation disposition は 3 値:** `accepted_non_material` / `blocking` / `unresolved`。close できるのは **事前に pin 済みの `accepted_non_material`** だけ。`blocking` / `unresolved` が 1 件でも pin されていれば `E_DEVIATION_NOT_ACCEPTED`。
6. **tool 分離:** `pr4_pf0h_close`（成果物生成）と `pr4_pf0h_verify`（独立再検証）を別 module・別実装にする。verify は close のコードを使わず（共有するのは PF0B の commitment 関数だけ）、入力から事実を再導出して 3 成果物と突き合わせる。自己検証だけにはしない。
7. **PF0H1 での絞り込み（fail closed 側）:** waive できる deviation class は `extra_run`（cadence）のみ。`skipped_run` / `report_missing` / `orphan_payload` / `tool_sha` の waive は実装しない（params に書くと `E_PARAMS_INCOMPLETE`）。reports の不一致・欠落、孤児 payload、tool sha の不一致はすべて STOP。deviation の pin は `deviation_file_name`（deviation dir 内の名前）と `deviation_file_sha256` を持つ。cadence は「JST 暦日に baseline を除く run が 2 回以上ある日は、n−1 件以上の pin が必要」で判定し、run が 0 回の日は STOP にせず `cadence_gap_days` として件数だけ記録する。
8. 実装: `apps/desktop/src-tauri/src/services/pr4_pf0h_close.rs` / `pr4_pf0h_verify.rs` / `pr4_pf0h_tests.rs`（test ビルドだけ・手動専用）。synthetic test 25 件（実行 25 件 + manual 1 件 ignored）（ケース T01〜T48 のうち、T09・T15・T35 の一部は r2 で意味が変わる／統合した）が通過。実 H2 は未読・未実行。
9. **（追加裁定 1）version 文字列は provenance metadata のみ。** `eligibility_rule_version`（`pr4-h2-enrollment-rules-2/enrollment_rule_sha256=…`）は authority として解釈しない。authoritative な membership は PF0B の `membership_commitment` の出力そのもの。version 文字列は照合（再計算の入力）にだけ使い、これを根拠にした判断・条件分岐をしない。
10. **（追加裁定 2）waive の範囲:** waive できるのは、**事前に pin 済みで membership に影響しない `extra_run` deviation だけ**。chain 破断・payload 欠落・checkpoint rollback・`report_missing`・`orphan_payload`・`tool_sha` などは **絶対に waive 不可**（実装済みの挙動どおり）。
11. **（追加裁定 3）hard 運用条件:** close 後は次の scheduled observation を走らせない。**close 検知の直後、次の定刻より前に日次 schedule を停止する**ことは契約上の必須条件で、満たされなければ close は通らない（`E_POSTCLOSE_CHECKPOINT`）。停止の確認（schedule 無効化の記録）を close tool 実行の前提手順に置く。
12. **（追加裁定 4）欠落日（MISSED）の扱い — 選択肢の検討記録。決定は項目 14。** 事実: 2026-10-09 の extra run（deviation）、2026-10-10 の scheduled run は MISSED（未実行・遡及 backfill なし）。この MISSED は **ledger にも checkpoint にも現れない**（`missed` record を書く run 自体が走っていないため）。現実装の V11 は「run のある JST 暦日」だけを突合し、run の無い日は `cadence_gap_days`（件数）として記録するのみで STOP にも開示にもならない。
   - **選択肢 A（現状）:** cadence は run のある日だけを突合。欠落日は件数だけ記録。長所: 変更不要・membership に影響しない。短所: 欠落日がどの日か、deviation log と照合できない。未知の欠落が見逃される。
   - **選択肢 B:** pin できる class に `skipped_run` を追加。**disclosure 専用（waive しない）**で、pin 内容は `jst_date`（欠落日）と deviation file sha・`accepted_non_material`。tool は (a) 宣言日に checkpoint が存在しないこと、(b) 実際の欠落日集合（最初と最後の run の間で run の無い JST 暦日）== pin された `skipped_run` の日付集合であること、を検査する。未宣言の欠落日・実在する日の宣言は STOP（`E_UNDECLARED_GAP` / `E_DEVIATION_MISMATCH`）。`H2_CLOSE.json` に `gap_days_disclosed` を出す。
   - **選択肢 C:** 欠落日を STOP にする（全日 run 必須）。**非推奨**: rules-2 は「実行できなかった run は missed として残し遡及登録しない」であり、欠落そのものは規則内の事象。close を止める理由にならない。
   - **推奨: B。** 欠落は membership を変えない（遡及登録しないので、その日に現れた作品は次の run で観測され `first_seen_at` が遅れるだけ。選択は live 判定のみで性能と無関係）が、「欠落日は網羅的に開示済み」を cadence 突合の対象にすることで、extra run と対称に、deviation log の網羅性を検査できる。実装は `check_deviations` の cadence 集計に日付集合の等式検査を足す小変更（waive の経路は増えない）。
   - 検査できないこと: 最後の run より後の欠落（close の run が最後）、および T0 当日より前。これらは対象外。
   - **本 gate では未実装。** B を採るなら PF0H1 の追修正（pin schema に `skipped_run` を `jst_date` 付きで追加、テスト: 宣言あり PASS／未宣言 STOP／実在日の宣言 STOP／日付ずれ STOP）。
13. **（追加裁定 5・実装済み）tool SHA は集合ではなく時系列で検証する。** checkpoint seq 1〜3 は OLD `183e0c73…e3213` のみ、seq 4 以降は NEW `36f869f6…18d08` のみ。seq 4 以降に OLD へ戻る・seq 3 以前に NEW が現れる・unknown SHA はすべて `E_TOOL_SHA`（waive 不可）。ledger の各 record（baseline / enroll）は、それを含む最初の checkpoint の seq の期待値と一致しなければならない。params の `enrollment_tool_sha_timeline` はこの 2 区間と完全一致し、`tool_transition_provenance`（PF0E-3 main `69d150d6…aebd9` / 元 commit `98e3917f…6a13b` / DAILY FREEZE AMENDMENT 2 `f4a2eb6d…2a0263`）も固定値と一致しなければ `E_PARAMS_INCOMPLETE`。`allowed_enrollment_tool_sha256`（集合）は廃止。
14. **（追加裁定 5・実装済み。項目 12 の選択肢 B を、この 1 件に限定した形で採用）** 2026-10-10（JST。scheduled observation #2）の MISSED は ledger に `missed` record が無い。external audit note の SHA `5f809726…68b68` を **唯一の historical exception** として pin する（type = `missed_without_ledger_record`、disposition = `accepted_non_material`、membership_impact = none、checkpoint_impact = none、backfill = forbidden）。V11 は「run のある日」に加え、**最初と最後の run の間で run の無い JST 暦日の集合 == pin 済み exception の日付集合**であることを検査する。それ以外の ledger 外の欠落日は `E_UNDECLARED_GAP`、exception を pin したが当日に run が実在する場合は `E_DEVIATION_MISMATCH`、exception が 2 件以上・別日付・SHA/属性の不一致は `E_PARAMS_INCOMPLETE`。audit note の本文は tool が読まない（SHA を値として pin。本文の保管・検証は audit 側）。**V6(c) chain coverage は hard のまま**で、この exception は V11 の cadence completeness だけに作用する。日付を authority とする（10/09 = #1 PASS、10/10 = #2 MISSED、10/11 = #3）。
15. **（最終条件 A・実装済み）** authoritative な membership commitment は PF0B の `membership_commitment` と byte-for-byte 一致する（close の出力と verify の受理値の両方をテストで固定。`unit_set_sha256` / `unit_input_sha256` / version 文字列は補助 provenance）。
16. **（close 前の audit-side preflight・必須）** exception を pin した params では、close 実行前に audit 側で `PR4_PF0E_MISSED_NOTE_20261010.json` の実在 → SHA-256 再計算 → `5f809726…68b68` との一致を確認し、その結果の SHA を close tool に渡す（`CINEMANTIS_PR4_PF0H_EXCEPTION_NOTE_PREFLIGHT_SHA256`）。未指定・不一致は `E_EXCEPTION_PREFLIGHT`。値は `H2_CLOSE.json` の `provenance.cadence_exception_preflight_sha256` に記録され、verify が確認する。tool は note の本文を読まない・解釈しない。
17. **（取り込み順序）** 0aca746 ベースのままでは main へ入れない。commit → main `69d150d` へ rebase → diff 確認 → `cargo test --lib pr4_pf0h` と関連回帰 → 再度 STOP（merge 承認待ち）。PF0E-3 で `record_missed` の works_sequence は修正済みのため、§7 Q1 の waiver 拡張は不要。

## 0. 目的と原則
H2 が 946 eligible に達したとき、**新しい設計判断をせず**、事前に凍結した close tool（実装は次 gate = PF0H1）を **1 回**実行するだけで、primary FINAL holdout の membership を閉じて固定する。

1. close tool は **読み取り専用**。ledger / checkpoint / payload / reports を書き換えず、production DB・snapshot・matcher を開かない（ソース走査テストで固定）。書くのは新規の出力ディレクトリの 3 ファイルだけ。
2. 判断は **事前固定 params**（§2）と **fail closed の STOP** だけで決まる。「少し違うが通す」は無い。例外は §5 の pin 済み deviation だけ。
3. 出力は件数・hash・版名のみ。unit key・work id・題名・payload の中身・判定・性能を含めない（`assert_report_clean` と同型の検査）。
4. PASS しなかった場合、3 ファイルは **1 つも作らない**。代わりに `H2_CLOSE_STOP.json`（失敗した check id と件数のみ）を書く。部分出力は無い。
5. membership の変更・除外・追加は、PASS 後も STOP 後も禁止（rules-2: removal / dropout FORBIDDEN）。STOP は「close を認めない」であって、「membership を直す」権限ではない。

## 1. 想定する正常状態（PASS の唯一の姿）
| 項目 | 値 |
|---|---|
| `enroll` record 数（= h2_enrolled_count） | **946**（`baseline` 1 件 + `enroll` 946 件 + `close` 1 件 + 任意の `missed`） |
| `close` record | ちょうど 1 件、ledger 最後の record、`enrolled_count = 946` |
| 947 件目以降 / close 後の record | 0 件 |
| ledger hash chain | seq 1..N 連続、`prev_commitment` / `record_commitment` 全再計算一致 |
| checkpoint chain | `0001.json`..`NNNN.json` 連番、全 `prev_checkpoint_sha256` 一致、各 checkpoint が ledger と整合（§3 V5） |
| payload | 946 unit それぞれの `matcher_input_sha256` に対応する `payloads/<sha>.json` が存在し、canonical 再計算 hash 一致 |
| duplicate | unit key の重複 0、baseline 所属 0、`id <= baseline.works_sequence` 0 |
| provenance | rule sha・tool sha・T0・baseline が pin 済み値と一致。未宣言の deviation 0 |

## 2. 事前固定 params（`H2_CLOSE_PARAMS.json`、close より前の別 gate で凍結）
close 時に決めることを無くすため、判断材料をすべて close 前に pin する。**値は監査側が既に保持している freeze 記録から転記し、ledger を読んで作らない。** 値が未確定のものは schema 上 `required` で、未 pin なら tool は起動時に STOP（`E_PARAMS_INCOMPLETE`）。

| 項目 | 内容 |
|---|---|
| `contract_version` | `pr4-pf0h0-close-1` |
| `expected_enrolled_count` | `946`（rules-2 の `operational_target_eligible` / `CLOSE_AT`） |
| `enrollment_rule_version` / `enrollment_rule_sha256` | `pr4-h2-enrollment-rules-2` / audit freeze の値。全 checkpoint の `enrollment_rule_sha256` と一致必須 |
| `allowed_enrollment_tool_sha256[]` | audit freeze の tool sha（`0aca746` の `pr4_pf0e.rs` のソース全体 sha）。全 record の `enrollment_tool_sha256`、全 checkpoint の値が、この集合の要素であること。集合は 1 要素が原則 |
| `t0_utc` | `2026-10-07T22:23:21Z`（baseline record の `observed_at`、checkpoint 0001 の `run_utc` と一致必須） |
| `baseline` | `baseline_count` / `baseline_membership_sha256` / `works_sequence` / `snapshot_sha256`（audit freeze の値） |
| `cadence` | `DAILY`、slot = 07:30 JST、判定日 = JST 暦日（§3 V11） |
| `auditor_anchors[]` | 監査側が daily report から控えた `{checkpoint_seq, checkpoint_sha256}`（複数可）。全 anchor がこの chain の prefix に含まれること |
| `deviations[]` | §5。pin 済みの deviation だけが許容される。**`PR4_PF0E_DEVIATION_20261009_EXTRA_RUN` を含む** |
| `known_tool_quirks[]` | §7 の既知の挙動のうち tool が許容するもの（id 固定） |

**実行時に渡す引数は 1 つだけ:** `CLOSE_ANCHOR_CHECKPOINT_SHA256`。close を実行した run の report（count-only、`closed=true`, `total_enrolled=946`, `checkpoint_sha256`）に載る値を、監査側が中継する。tool は、この sha が chain 内の「close record を最初に含む checkpoint」の sha と一致することを確認する（§3 V6）。これは設計判断ではなく転記で、手元の ledger を丸ごと作り直しても通らない外部アンカーになる。

## 3. 検証（V1〜V16）— 全部 PASS で初めて出力する
STOP コードは `E_<名前>`。**H** = hard（waive 不可）、**W** = pin 済み deviation の範囲でのみ waive 可。

| # | 検証 | 方法 | STOP |
|---|---|---|---|
| V0 | 入力の構造 | ledger dir に既知ファイル（`ledger.jsonl` / `baseline_ids.json` / `payloads/` / `reports/`）以外が無い。checkpoint dir は `NNNN.json` のみ。両 dir は入れ子でない。出力 dir は存在しないか空 | `E_UNKNOWN_FILE` H |
| V1 | **ledger 全 chain 再検算** | 先頭から全 record: `seq` 連続、`prev_commitment` 一致、`SHA256(canon(record − commitment) + "\n" + prev)` 再計算一致、時刻形式・単調性、先頭は `baseline` 1 件のみ。PF0E の `verify_ledger` と同じ規則を **close tool 内で独立に再実装**（PF0E を import しない。両者が一致することを test で固定） | `E_LEDGER_CHAIN` H |
| V2 | close record の位置 | `close` がちょうど 1 件、かつ最後の record。それより前に `close` 無し、後に任意の record 無し。`close.enrolled_count == 946` | `E_NO_CLOSE` / `E_RECORD_AFTER_CLOSE` / `E_CLOSE_COUNT` H |
| V3 | enrolled unit count | `enroll` record 数 == `expected_enrolled_count`（945 も 947 も STOP）。`close.enrolled_count` と一致 | `E_COUNT` H |
| V4 | duplicate / identity | `evaluation_unit_key` が `work:<正整数>` 形式で全 unit ユニーク。baseline 集合と交わらない。`id > baseline.works_sequence`。baseline_ids の hash・件数が baseline record・params と一致。同一 `matcher_input_sha256` が複数 unit に出るのは許容（件数のみ manifest に `shared_payload_unit_count` として記録） | `E_DUPLICATE_UNIT` / `E_BASELINE_OVERLAP` / `E_IDENTITY_ASSUMPTION` / `E_BASELINE_MISMATCH` H |
| V5 | **checkpoint 全 chain 再検算** | 連番・`prev_checkpoint_sha256`・`seq`・`sha256(canon(checkpoint))` を全件再計算（ファイルの raw byte ではなく canonical 再計算 hash を使う）。各 checkpoint について: `record_count` は非減少、`ledger_head_sha256` == ledger の `record_count` 番目の `record_commitment`、`enrollment_rule_sha256` == params、`enrollment_tool_sha256` ∈ `allowed_enrollment_tool_sha256`（W）、`works_sequence` は init/enroll 間で非減少（`missed` の扱いは §7-Q1）。checkpoint 0001 は baseline 1 件 | `E_CHECKPOINT_CHAIN` / `E_CHECKPOINT_LEDGER_MISMATCH` H、`E_TOOL_SHA` W |
| V6 | **close anchor / rollback** | (a) 引数の `CLOSE_ANCHOR_CHECKPOINT_SHA256` が「close record を含む最初の checkpoint」の sha と一致。(b) `auditor_anchors[]` の各 `{seq, sha}` が chain に存在。(c) **最後の checkpoint の `record_count` == ledger の全 record 数**（ledger が checkpoint より先に進んだ未カバー末尾を許さない）。(d) close 後の checkpoint は **no-op のみ**（`record_count` / `ledger_head` が close checkpoint と同一）で、その数を `postclose_noop_checkpoints` に記録 | `E_ANCHOR_MISMATCH` / `E_CHECKPOINT_ROLLBACK` / `E_UNCOVERED_TAIL` / `E_POSTCLOSE_CHANGE` H |
| V7 | payload existence / hash | 各 `enroll` について `payloads/<matcher_input_sha256>.json` が存在し、parse → `canon` 再計算 hash == ファイル名 == record の値。`enrollment_context_sha256 == sha_of(enrollment_context)`。`enrollment_context.first_seen_at == record.first_seen_at`。**存在する payload の unit 数 == 946**（ファイル数ではない）。payload の中身は構造検査（parse 可否・hash）以外で解釈せず、出力に出さない | `E_PAYLOAD_MISSING` / `E_PAYLOAD_HASH` / `E_CONTEXT_HASH` H |
| V8 | payload の余剰 | どの enroll record も参照しない payload ファイルが 0（crash 窓の孤児も含め、無い状態が正常） | `E_ORPHAN_PAYLOAD` W（pin 済み hash 一覧のみ） |
| V9 | membership commitment 再計算 | §4.2。manifest の値は **tool が 2 回独立に計算して一致**（ledger 順 / key 順の 2 経路） | `E_COMMITMENT_SELF_CHECK` H |
| V10 | enrollment rule / tool provenance | 全 record の `enrollment_rule_version == pr4-h2-enrollment-rules-2`、`enrollment_context.payload_schema_version == pr4-h2-input-payload-1`、`canonicalization_version == pr4-canonical-json-1`、`evidence_class_at_enrollment == "live"`、`enrollment_tool_sha256 ∈ allowed`。baseline record も同様 | `E_RULE_PROVENANCE` H、`E_TOOL_SHA` W |
| V11 | cadence / deviation 突合 | §5。各 JST 暦日の run 数と時刻を checkpoint の `run_utc` から計算し、slot 外・日内 2 回目以降の run を **すべて** `deviations[]` と突合。未宣言は STOP | `E_UNDECLARED_DEVIATION` H、`E_CADENCE_*` W |
| V12 | T0 provenance | baseline record の `observed_at == t0_utc`、checkpoint 0001 の `run_utc == t0_utc` かつ `kind == baseline`、baseline の `works_sequence` / `baseline_membership_sha256` / `baseline_count` が params と一致。`first_seen_at >= t0_utc` が全 enroll で成立（`>` ではなく T0 run 自身は enroll しない: T0 run の seq は 1 のみ） | `E_T0_MISMATCH` H |
| V13 | reports 突合（第三の証人） | `reports/*.json` の各ファイル: `checkpoint_sha256` が chain 内に存在、同 seq の `record_count` / `ledger_head_sha256` / `enrollment_tool_sha256` が一致、`closed` / `total_enrolled` が整合。checkpoint ごとに report がちょうど 1 つ | `E_REPORT_MISMATCH` / `E_REPORT_MISSING` W |
| V14 | 時刻の整合 | `close.observed_at` ≥ 最後の enroll の `first_seen_at`、close を含む run の `run_utc == close.observed_at`。close 時刻は freeze 実行時刻より過去 | `E_TIME_ORDER` H |
| V15 | 入力の不変 | tool 起動時と終了時で、ledger dir / checkpoint dir の全ファイル hash 一覧が同一（読み取り中の daily run との競合も検出）。**freeze の前に daily schedule を停止する運用が前提**（§6） | `E_INPUT_CHANGED` H |
| V16 | 出力の清浄 | 3 ファイルに `work:`・数値 work id 配列・unit 配列・題名・パス・判定語が無い（unit 単位の配列を持たない）。値はスカラーと hash のみ | `E_OUTPUT_NOT_CLEAN` H |

**対象外（意図的）:** (i) H1 / C5c.5 / 過去 GT との dedup の再検算（rules-2: 「登録時に 1 回だけ。再適用しない」）。(ii) evidence hash（local / embedded / decision）が現行 parser と一致するかの再計算（PF0E: audit aid であり、不一致でも membership を外さない）。(iii) DB・matcher・label・GT の参照。

## 4. 成果物（出力ディレクトリは新規作成のみ・上書き禁止・全部を temp に書いてから最後に原子的に rename）
### 4.1 `H2_CLOSE.json` — close manifest（人が読む要約と検証結果）
`protocol: pr4-pf0h0-h2-close-1`、`status: "PASS"`、`generated_at`（このファイルだけの非決定値）、`contract_version`、`close_tool_sha256` / `close_tool_head`、`params_sha256`、`inputs`（ledger / checkpoint の各 head と件数、`close_anchor_checkpoint_seq/sha256`）、`counts`（`record_count`, `baseline_count`, `enrolled_count=946`, `missed_count`, `postclose_noop_checkpoints`, `payload_units_present=946`, `shared_payload_unit_count`, `duplicate_unit_count=0`）、`checks[]`（V0〜V16 ごとに `{id, result: PASS|WAIVED, waiver_ref?}`）、`provenance`（rule / tool / T0 / baseline の照合結果）、`deviation_exposure[]`（§5）、`limits`（"count-only; no machine output; no performance"）。
### 4.2 `FINAL_MEMBERSHIP_COMMITMENT.json` — membership の commitment（完全に決定的。時刻・path を含まない）
- `unit_set_sha256` = SHA256( `pr4-pf0h0-final-membership-1\n` + `pr4-h2-enrollment-rules-2\n` + 各 `work:<id>` を **バイト順（辞書順。PF0A と同じ。数値順ではない）** に `key\n` )。
- `unit_input_sha256` = SHA256( `pr4-pf0h0-final-membership-input-1\n` + 同順で `key\t<matcher_input_sha256>\t<enrollment_context_sha256>\n` )。所属と登録時 input の両方を縛る。
- `unit_count = 946`、`ledger_head_sha256`、`close_record_commitment`、`baseline_membership_sha256`、`enrollment_rule_sha256`、`h1_membership_commitment_sha256`（`22cc00a8…983e031` — **H1 は primary に使わない**ことの記録のみ）、`deviation_subset_commitments[]`（§5。deviation run 由来 unit だけの `unit_set_sha256`。0 件なら空）。
- キー一覧は出力しない（ledger が正本。PF0A と同じ「キーは持ち出さない」）。
### 4.3 `H2_LEDGER_FINAL_FREEZE.json` — ledger の最終凍結記録
`ledger_jsonl_sha256` + `byte_length`（raw bytes。以後この hash が正本）、`record_count`、`ledger_head_sha256`、`baseline_ids_sha256`、`payload_tree`（`file_count`, `payload_tree_sha256` = ファイル名（= content hash）を昇順に `name\n` 連結した hash）、`checkpoint_chain`（`count`, `head_sha256`, `files_digest_sha256`）、`reports_digest_sha256`、`tool_provenance`（PF0E tool sha / rules sha / `tool_head`）、`frozen_state: "CLOSED"`、`next_allowed_actions: ["machine_output_freeze"]`、`still_forbidden: ["performance_view", "membership_change", "enroll_run", "gt_review_before_machine_output_freeze"]`。
- 3 ファイル相互の整合: `FINAL_MEMBERSHIP_COMMITMENT.ledger_head_sha256` == `H2_LEDGER_FINAL_FREEZE.ledger_head_sha256` == `H2_CLOSE.inputs.ledger_head`。`verify-artifacts` モード（入力を再読込して 3 ファイルを再導出し、`generated_at` 以外が byte 一致）を監査用に同梱する。

## 5. deviation log の扱い（`PR4_PF0E_DEVIATION_20261009_EXTRA_RUN` を含む）
**分類:** rules-2 の「裁量での追加 scan・間引きは禁止」に対する **手続き違反**であって、それ自体は ledger の整合性違反ではない（追加 run も同じ決定的規則 = live のみ・work id 昇順・hash chain で記録される）。したがって:
1. deviation log は監査側の別ファイル。**close 前の gate で sha256 を `H2_CLOSE_PARAMS.deviations[]` に pin**し、各エントリに `deviation_id`, `class`（`extra_run` / `skipped_run` / `report_missing` / `orphan_payload` / `tool_sha`）, `run_utc`（該当 run の UTC）, `declared_checkpoint_seq`, `declared_enrolled_in_run`（その run で登録された件数。不明なら `null` ではなく調査後に確定した整数）, `auditor_disposition: "accepted_with_disclosure"` を持たせる。disposition が未確定／不受理なら STOP（close を認めない）。
2. tool は pin 済みエントリごとに実データと突合する: (a) deviation ファイル sha == pin、(b) `run_utc` と `declared_checkpoint_seq` が chain 上の同一 checkpoint を指す、(c) 実際の `first_seen_at == run_utc` の `enroll` 件数 == `declared_enrolled_in_run`、(d) その run の kind / tool sha が宣言と一致。1 つでも違えば `E_DEVIATION_MISMATCH` H。
3. **未宣言の extra run（日内 2 回目以降、または slot 外）・未宣言の欠落 run は `E_UNDECLARED_DEVIATION`（STOP）。** deviation log は網羅的でなければならない（無いことの証明は cadence 突合で行う）。
4. **membership への影響:** deviation run で登録された unit も **member のまま**（rules-2: 登録後の除外禁止。除外は「事後の選択」になる）。代わりに開示する: `H2_CLOSE.deviation_exposure[] = {deviation_id, class, enrolled_in_run, share_of_final}` と、`FINAL_MEMBERSHIP_COMMITMENT.deviation_subset_commitments[]`（その unit 部分集合の commitment）。後続の事前登録済み感度分析（deviation 由来 unit を除いた集計）を、キー一覧を再導出せずに検証可能にするため。感度分析の採否は本契約の範囲外（性能を見る前に別 gate で決める）。
5. extra run が enrollment に与えた効果の性質（記録のみ）: enrollment event は「scheduled observation で live と観測された時点」なので、extra run 由来の unit は `first_seen_at` が次の定刻より早い、または定刻 run だけでは観測されなかった可能性がある。この差は **作品の性能と無関係に生じる**ことが機構上言える（live 判定は label / matcher 出力を読まない）が、その証明は本契約では行わず、開示にとどめる。
6. deviation が `class` 外（hard 検査の失敗＝chain 破断・件数違い・payload 欠落など）を waive することは **できない**。

## 6. 運用手順（人の判断を入れない順序）
1. 日次 run が `closed=true, total_enrolled=946` の report を出す（これが「946 到達」の唯一のシグナル。性能・件数以外は見ない）。
2. **日次 schedule を停止**（以後 enroll run を走らせない。close 後の no-op checkpoint は V6(d) が許容するが、増やさない）。
3. 監査側が report の `checkpoint_sha256` を中継（= `CLOSE_ANCHOR_CHECKPOINT_SHA256`）。
4. production を触らず、ledger / checkpoint dir の byte-copy（read-only の copy 元／作業 copy は別 dir）。close tool は copy に対して実行する。
5. close tool を **1 回** 実行 → PASS なら 3 ファイル、STOP なら `H2_CLOSE_STOP.json`。
6. 監査: `verify-artifacts` で再導出一致を確認 → 3 ファイルの sha を audit freeze に pin → commit。
7. STOP の場合: 原因は deviation か破損かを監査が判断（本 tool は再実行で通過を探さない。入力が変わらない限り同じ STOP を返す）。
**PASS 後に許されるのは machine output freeze の準備だけ。性能の閲覧・GT レビュー・membership 変更は引き続き禁止。**

## 7. 既存 PF0E tool に対して設計時に見つけた点（tool は変更しない。契約側で吸収／周知）
- **Q1（要確認・高）:** `record_missed`（`pr4_pf0e.rs:609`）は `Report.works_sequence` に **現在値ではなく `baseline_sequence`** を入れる。一方 `run`（`:697`）は `r.works_sequence < p.works_sequence` で STOP する。したがって **直前の checkpoint で `works_sequence` が baseline より増えた後に `missed` を記録すると、ledger に missed record が追記された後で STOP し、checkpoint が書かれない**（ledger が checkpoint より 1 record 先行）。次の run は `expected=(checkpoint の count, head)` で `count <= records` を許容するため進行は可能だが、**未カバー末尾**が残る。契約側は V6(c) で最終状態では未カバー末尾を許さない（= 該当すれば `E_UNCOVERED_TAIL`）。実運用で missed を使う予定が無い／使っていないかは監査側の記録で確認してほしい（私は ledger を見ていない）。tool 修正は本 gate の範囲外。
- **Q2:** close 後も日次 `enroll` run は走り続けうる（`enroll` は closed なら 0 件で return、`run` は no-op checkpoint と report を書く）。§6-2 で停止、V6(d) で no-op のみ許容。
- **Q3:** `tool_sha256()` はテストを含むソース全体の `include_str!` hash、`rules_sha256()` も `include_str!` なので、**改行コード（autocrlf）で変わりうる**。close tool は git から再計算せず、record / checkpoint に記録済みの値と params の pin だけを比較する。
- **Q4:** checkpoint chain の sha は canonical 再計算値で、ファイルの raw byte には依存しない（`write_checkpoint` の末尾改行はソース上のリテラル改行で OS 依存）。V5 は canonical 再計算のみ。`H2_LEDGER_FINAL_FREEZE.checkpoint_chain.files_digest_sha256` は raw byte を使うため、**監査用コピー作成時に改行変換をしない**こと。
- **Q5:** payload は content-addressed なので同内容の unit は 1 ファイルを共有しうる。「payload 946 件」は unit 単位（V7）で、ファイル数ではない。
- **Q6（残余リスク）:** ledger+checkpoint+reports を **全部一括で作り直した**場合は内部整合だけでは検知できない。検知は外部アンカー（`auditor_anchors[]` と close anchor、audit freeze の baseline／tool／rule sha）に依存する。daily report の `checkpoint_sha256` を監査側が継続的に控えていることが前提。

## 8. Synthetic test plan（全て合成。実 H2・実 production に触れない。テストは一時ディレクトリで `enroll_with_cap` と手組み record から ledger を生成。PF0E の本番 cap 946 を params に使う）
共通: 各ケースで「期待 status」「STOP コード」「3 ファイルが作られない（STOP 時）／作られる（PASS 時）」「入力 dir の hash 不変」を検証。

| # | ケース | 生成方法 | 期待 |
|---|---|---|---|
| T01 | **exactly 946** | baseline + 946 enroll + close、checkpoint 整合 | **PASS**、3 ファイル、`unit_count=946`、再実行で `generated_at` 以外 byte 一致 |
| T02 | 945 | baseline + 945 enroll（close 無し） | STOP `E_NO_CLOSE`（`E_COUNT` も） |
| T03 | 945 + 偽 close | 945 enroll + `close(enrolled_count=945)`、chain 正しく再計算 | STOP `E_COUNT` |
| T04 | 947 | 947 enroll + `close(947)` を合法 chain で手組み | STOP `E_COUNT` |
| T05 | 946 enroll、close 無し | close を除いた ledger（checkpoint も整合） | STOP `E_NO_CLOSE` |
| T06 | close の後に enroll 追加 | 合法 chain で `close` 後に enroll を 1 件 | STOP `E_RECORD_AFTER_CLOSE` |
| T07 | close の後に missed 追加 | 同上 `missed` | STOP `E_RECORD_AFTER_CLOSE` |
| T08 | close 後に checkpoint が変化 | close 後の checkpoint で `record_count`／head を変更 | STOP `E_POSTCLOSE_CHANGE` |
| T09 | close 後の no-op checkpoint（1〜3 本） | PF0E の通常動作で no-op 日次 run | PASS、`postclose_noop_checkpoints` = 本数 |
| T10 | duplicate unit | 同一 `work:<id>` を 2 回（chain 再計算済み。count が 946 になるよう調整） | STOP `E_DUPLICATE_UNIT` |
| T11 | baseline との重複 | baseline 所属 id を enroll | STOP `E_BASELINE_OVERLAP` |
| T12 | `id <= works_sequence` | baseline の works_sequence 以下の id を enroll | STOP `E_IDENTITY_ASSUMPTION` |
| T13 | payload 欠落（1 件） | 1 ファイル削除 | STOP `E_PAYLOAD_MISSING` |
| T14 | payload 内容改変 | 1 ファイルの 1 byte 変更 | STOP `E_PAYLOAD_HASH` |
| T15 | payload 共有 | 2 unit が同一 payload（実現可能な範囲で手組み） | PASS、`shared_payload_unit_count` 反映 |
| T16 | 孤児 payload | 余分な payload ファイルを 1 つ追加 | STOP `E_ORPHAN_PAYLOAD`（pin 済み hash なら WAIVED） |
| T17 | ledger 改変（中間 record の 1 フィールド） | in-place 編集 | STOP `E_LEDGER_CHAIN` |
| T18 | ledger 中間削除／並べ替え | 行削除・swap | STOP `E_LEDGER_CHAIN` |
| T19 | ledger 末尾削除（close を削る） | 末尾 1 行削除 | STOP `E_CHECKPOINT_LEDGER_MISMATCH`／`E_NO_CLOSE` |
| T20 | **ledger 全再計算の偽造**（改変点以降の chain を全て再計算） | chain は内部整合 | STOP `E_CHECKPOINT_LEDGER_MISMATCH`（checkpoint の head と不一致） |
| T21 | ledger+checkpoint 一括再計算 | 両 chain とも整合する偽造 | STOP `E_ANCHOR_MISMATCH`（外部 anchor） |
| T22 | **checkpoint rollback**（末尾 N 本削除、ledger はそのまま） | checkpoint 末尾削除 | STOP `E_ANCHOR_MISMATCH`／`E_UNCOVERED_TAIL` |
| T23 | checkpoint rollback + ledger も巻き戻し | 両方を close 前に戻す | STOP `E_NO_CLOSE`／`E_ANCHOR_MISMATCH` |
| T24 | checkpoint 改変／欠番／並べ替え | 1 フィールド変更・`0005.json` 削除 | STOP `E_CHECKPOINT_CHAIN` |
| T25 | checkpoint の head 不一致 | checkpoint の `ledger_head_sha256` だけ改変（chain 再計算） | STOP `E_CHECKPOINT_LEDGER_MISMATCH` |
| T26 | ledger が checkpoint より先行 | 最後の checkpoint を未生成にする（Q1 の再現） | STOP `E_UNCOVERED_TAIL` |
| T27 | rule sha 不一致 | checkpoint の `enrollment_rule_sha256` を差し替え | STOP `E_CHECKPOINT_CHAIN`／`E_RULE_PROVENANCE` |
| T28 | tool sha 不一致（未 pin） | 1 つの checkpoint／record の tool sha を別値に | STOP `E_TOOL_SHA`（pin 済みなら WAIVED だが deviation 突合） |
| T29 | T0 不一致 | baseline `observed_at` を params と別値に | STOP `E_T0_MISMATCH` |
| T30 | baseline 不一致 | baseline_ids の 1 件変更 | STOP `E_BASELINE_MISMATCH` |
| T31 | **extra run（宣言済み・整合）** | 日内 2 回の run、deviation を pin、件数一致 | PASS、`deviation_exposure` と `deviation_subset_commitments` が出る |
| T32 | extra run（未宣言） | 同じ ledger で deviation を pin しない | STOP `E_UNDECLARED_DEVIATION` |
| T33 | extra run（宣言したが件数不一致） | `declared_enrolled_in_run` を 1 ずらす | STOP `E_DEVIATION_MISMATCH` |
| T34 | deviation ファイル sha 不一致 | log の内容を 1 byte 変更 | STOP `E_DEVIATION_MISMATCH` |
| T35 | disposition 未確定 | `auditor_disposition` が受理でない | STOP |
| T36 | deviation で hard 失敗を waive しようとする | deviation に `E_LEDGER_CHAIN` を指定 | params 検証で STOP `E_PARAMS_INCOMPLETE` |
| T37 | close anchor 不一致 | 引数の sha を 1 文字変更 | STOP `E_ANCHOR_MISMATCH` |
| T38 | 未知ファイル | ledger dir に stray ファイル | STOP `E_UNKNOWN_FILE` |
| T39 | report 不一致／欠落 | report の `record_count` 変更・1 件削除 | STOP `E_REPORT_MISMATCH`／`E_REPORT_MISSING`（pin 済みなら WAIVED） |
| T40 | 入力が実行中に変化 | tool 実行中にファイル追加（フック） | STOP `E_INPUT_CHANGED` |
| T41 | 上書き拒否・原子性 | 出力 dir が非空／途中で STOP | 既存を触らない、部分出力無し |
| T42 | 決定性 | 2 回実行 | `FINAL_MEMBERSHIP_COMMITMENT`・`H2_LEDGER_FINAL_FREEZE` が完全一致、`H2_CLOSE` は `generated_at` のみ差 |
| T43 | commitment の感度 | unit を 1 件入替／順序入替 | 入替で `unit_set_sha256` 変化、ledger 順入替では不変（key 順で固定）、`unit_input_sha256` は payload/context 変更で変化 |
| T44 | 出力の清浄 | 出力に `work:`・題名・パス・配列が無い | PASS |
| T45 | ソース走査 | close tool のソースに `rusqlite`／snapshot open／matcher／label／GT／`pr4_pf0e` import が無い、書き込みは出力 dir のみ | PASS |
| T46 | PF0E との一致 | 独立実装の V1 を PF0E の `verify_ledger` と同一合成 ledger（正常・各種破損）で比較 | 判定が一致 |
| T47 | params 未 pin | required を欠く／`expected_enrolled_count != 946` | STOP `E_PARAMS_INCOMPLETE` |
| T48 | 入力 dir の不変 | 全ケースで実行前後の入力 hash | 不変 |

## 9. 監査に判断してほしい点（設計上の open decision）
1. **V6(c)**（最後の checkpoint が ledger 全 record をカバー）を hard とする案でよいか。Q1 が現実に起きた場合、close は止まる（deviation で waive 不可）。代案: `uncovered_tail` クラスを waivable にして pin 済み deviation で許す。
2. **close 後の no-op checkpoint** を許容する案（現案）か、「close 検出の時点で schedule 停止を hard 前提とし、1 本でも後続があれば STOP」にするか。
3. **deviation_subset_commitments** を出す案（現案）か、開示（件数）だけにするか。
4. extra run に対する disposition の語彙（`accepted_with_disclosure` のみ許す現案でよいか）。
5. close tool の配置（PF0H1 で `services/pr4_pf0h.rs` の test-build only・手動専用、PF0E と同じ流儀）と、`verify-artifacts` を同一 tool に入れるか。

## 10. この gate で行っていないこと
実 H2 ledger / checkpoint / payload / deviation log / `CineMantis-audit\pr4\pf0e` 配下の読取、機械出力の生成、性能の閲覧、production DB・メイン checkout・PF0E tool・rules の変更、commit・push。ここで止めて設計レビュー待ち。
