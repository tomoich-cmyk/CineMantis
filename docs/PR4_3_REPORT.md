# PR4-3: 診断 conditional AUTO precision — 実装報告

状態: **実装・テスト済み。commit していない。** 実データでの検証（read-only の実 DB）は別 gate で、ここでは行っていない。holdout は封印のまま。FINAL の昇格指標は NOT_RUN。policy freeze は未定義。
前提: PR4-0 / PR4-1 / PR4-2 CLOSED、PR4-2A の bridge `READY`（SHA-256 `0f31ea9678cb7e078b82970fca3d9f3beb1338640ee8b9cfe1fe7e623cb8559c`）。HEAD `5241d62b4e42832e83740675a8731793c73d5ff0`。

## 変更したファイル

| ファイル | 内容 |
|---|---|
| `apps/desktop/src-tauri/src/services/pr4_diagnostics.rs` | PR4-1 の通常診断集計に、診断 GT（`diagnostic_gt` セクション）を実装。テストを 20 件追加 |
| `docs/PR4_3_REPORT.md`、`docs/PR4_3_FREEZE.txt`（新規） | この報告と凍結ハッシュ |

`mod.rs`・他のコード・migration・DB・matcher / rules / Jev の挙動・PR4-0 / 1 / 2 / 2A の凍結ファイルは変えていない。

## 実装したもの

`generate_report(conn, cohort, generated_at, head, gt: Option<&GtConfig>)`。`GtConfig` は bridge のパス・期待する SHA-256・正式な件数（母集団 / resolved / deferred）を持つ。`GtConfig::production(path)` が凍結値（SHA `0f31ea96…`、50 / 44 / 6）を入れる。**bridge の絶対パスはコードに固定しない**（呼び出し側が渡す）。

1. **bridge の検証（パースの前に SHA を確認）**: 欠落 → `bridge_missing` / SHA 不一致 → `bridge_sha_mismatch` / スキーマ不正（余分・不足のキー、型、`DEFER` などの意味）→ `bridge_schema_invalid` / 準備ができていない（正式な件数が違う、DB 変更・holdout 読み取りの宣言が偽でない）→ `bridge_not_ready` / 同じ評価単位の重複 → `duplicate_bridge_unit` / formal な sample 群の外 → `cohort_boundary_failure`。いずれも `diagnostic_gt.status = BLOCKED`。
2. **GT の merge**（キー = `{sample_id, task_id, work_id}`。優先順位なし、latest-wins なし）: DB だけ → 含める / bridge だけ → 含める / **同じキー・同じ canonical GT → 1 件に畳む（`reviewed_run_id` が違っても結合し、両方の出典と値を provenance に残す。違いは件数 `reviewed_run_id_differs` に出す）** / **同じキー・違う canonical GT → `gt_source_conflict`（`reviewed_run_id` が同じでも違っても）**。`reviewed_run_id` は属性で、canonical GT ではない。DB に task がある bridge の単位は、キーの全成分を照合し、違えば `evaluation_key_mismatch`、holdout / 未対応の task を指せば `cohort_boundary_failure`。defer / 未解決は GT にならない。
3. **DB の信頼できる GT**: PR4-2 の allowlist（`review_confirm` / `review_pick_other` / `review_none`、strong、task・run・解決記録・protocol 版の整合、`tmdb_verified`、formal な sample、上書きされていない）。それ以外のラベルは理由別に数える（`untrusted_manual_gt_source` / `weak_label_not_gt` / `unknown_label_semantics` / `gt_sample_not_allowlisted` / `gt_provenance_incomplete` / `gt_superseded` / `gt_conflict` / `gt_multiple_units_per_work`）。監査 task に紐づくラベルの意味が未対応なら `unsupported_label_semantics` で BLOCKED。
4. **整合性（44 は分母ではない）**: 結合後の母集団・解決・保留の件数が、期待（50 / 44 / 6）と一致しなければ `gt_universe_mismatch`。他の blocker が既にあるときは、その結果としてのずれを重ねて出さない。
5. **機械側**: PR4-1 と同じ `read_verdicts`（verdicts が正）。matcher（rules-safe / rules-tags-shadow）ごとに別々に計算する。**AUTO だけ**が分母に入る。REVIEW / UNRESOLVED は分母外（PR4-1 の coverage / REVIEW / abstention には残る）。AUTO なのに選んだ identity が無い → `invalid_auto_output` で BLOCKED（正しさを推測しない）。正しさは `(media_type, tmdb_id)` の完全一致だけ。**none の GT に AUTO が候補を選べば誤り。** rank・top-3・score・題名・Jev・候補テーブルは参照しない（テストでソースを検査）。
6. **cohort**: PR4-1 の `select_runs` をそのまま使う（第 2 の実装なし）。GT の組み立ては、`select_runs` が開発用と分類し、holdout / 未対応の task が無い作品の課題だけが対象。ラベルはその作品の分だけを読む。holdout は件数だけで、GT・verdict・候補・ラベルの中身は出さない。`cohort_boundary_integrity = APPLICATION_VALIDATED_NOT_DB_ENFORCED` を維持。
7. **BLOCKED のとき** precision は出さず（`profiles = {}`・`precision_computed = false`）、分母を黙って減らして計算可能にしない。PR4-1 の通常セクションは BLOCKED でも出る。
8. **`diagnostic_gt` の出力**: `status`（PASS / BLOCKED）、`contract_version`、`bridge_version`、`bridge_sha256`、`blockers`、`gt_universe`（resolved / deferred / db_only / bridge_only / merged_same_gt / source_conflicts / db_unresolved_covered_by_bridge）、`excluded_gt`、`unresolved_gt_count`、`materialization`（読んだラベルの件数）、matcher ごとの `auto_evaluable_count` / `auto_correct_count` / `auto_incorrect_count` / `diagnostic_conditional_auto_precision`（分母 0 は `null`）/ `not_in_denominator`、`correctness_definition = exact_media_type_tmdb_id`、`top3 = NOT_APPLICABLE`。**GT の中身（作品 ID・TMDB ID）は出さない。件数だけ。**
9. **`final_promotion`** は常に `{status: NOT_RUN, reason: HOLDOUT_SEALED_POLICY_NOT_FROZEN}`。この出力は昇格判断に使えない。

A2（production）の cohort では `diagnostic_gt.status = NOT_APPLICABLE`（診断 GT は A1 だけで定義される）。

## 契約への amendment（監査の指摘による修正）

1. **`reviewed_run_id` は衝突の根拠にしない。** 初版は「同じキー・同じ GT でも `reviewed_run_id` が違えば `gt_source_conflict`」だったが、PR4-2A はキーを `{sample_id, task_id, work_id}` に定め、`reviewed_run_id` を意図的にキーから外した属性としている。属性の差だけで GT の衝突にするのは契約と整合しないので、次に直した（PR4-2A の契約 §4・§7(6) とスキーマの `x-semantic-rules`、外部 builder の参照 merge の 1 ケースにある「`reviewed_run_id` が違えば衝突」は、PR4-3 のこの規則で置き換える。凍結済みのファイルは書き換えない）。
   - 同じキー・同じ canonical GT・違う `reviewed_run_id` → **結合する。** 両方の出典（`TRUSTED_DB_GT` / `FROZEN_OFF_DB_GT`）と、それぞれの `reviewed_run_id` の値を provenance に残す。機械側の出力は DB の task が指す run のものを使う。
   - 同じキー・違う canonical GT → `gt_source_conflict`（`reviewed_run_id` が同じでも違っても。`media_type` だけの違いも衝突）。
   - 追加したテスト 2 件: `the_same_key_and_gt_with_a_different_reviewed_run_id_is_merged_and_both_refs_are_kept` / `the_same_key_with_a_different_canonical_gt_is_a_conflict_whatever_the_reviewed_run_id`。既存の衝突テストの「run の食い違いは衝突」の部分は、この規則に合わせて「GT が違えば衝突」に直した。
2. **A2 の `diagnostic_gt = NOT_APPLICABLE` は、凍結済みの契約に明示された範囲に従う（新しい意味論ではない）。** 根拠: PR4-0 の `PR4_AGGREGATION_CONTRACT.md`（`diagnostic_conditional_auto_precision` の節に「cohort は **A1 だけ**」）、`pr4_metric_contract.json`（`"cohort": "A1_development_audit のみ"`）、同 §2 の A2 の行（「ラベルなし・分布指標だけ」）、PR4-2 の分母の定義（「適格な評価単位（PR4-1 の A1 cohort）」）。`diagnostic_conditional_auto_precision` は、信頼できる C5c.5 の GT の母集団が A1 に属するため、A1 だけで定義される。A2 は「GT が足りないから 0 件の precision」とはせず、`NOT_APPLICABLE`（値を出さない）にする。`NOT_APPLICABLE` という status の名前は PR4-3 のレポートの表記で、契約の意味を足すものではない。

## PR4-1 からの置き換え

`diagnostic_gt` は PR4-1 の `{"status": "NOT_IMPLEMENTED_PR4_1"}` から置き換わった。`gt = None` のときは `BLOCKED`（`bridge_missing`）。PR4-1 のテスト 1 件（`final_promotion_is_never_run_…`）を、この新しい期待値に更新した（それ以外の PR4-1 のテストは変更なし）。`generate_report` の引数が 1 つ増えたが、呼び出し元はテストだけ。

## テスト

| 実行 | 結果 |
|---|---|
| `cargo test --lib pr4_diagnostics` | 47 passed / 0 failed（PR4-1 の 27 + PR4-3 の 20） |
| `cargo test --lib`（全件の回帰） | 486 passed / 0 failed / 8 ignored（もともと手動用の `#[ignore]`） |
| `cargo check --lib` | 通過（このモジュールの警告なし） |
| 凍結した実際の bridge（`CM_PR4_BRIDGE_PATH` で渡したとき） | SHA・スキーマの検証を通り、4 件の GT を読めた |

PR4-3 の 20 件が確かめること: exact AUTO → 正しい / 別 identity → 誤り / none + AUTO → 誤り / matcher を別々に計算 / defer・GT なし・REVIEW・UNRESOLVED は分母外 / 分母 0 → null / DB だけ・bridge だけ・同一の merge / 衝突 → BLOCKED（precision を出さない）/ bridge の SHA 不一致・欠落・スキーマ不正・未準備・sample 範囲外・重複 → BLOCKED / 件数が 43 相当・45 相当・母集団違い → `gt_universe_mismatch` / identity の無い AUTO → BLOCKED / キー不一致・holdout の task を指す bridge → BLOCKED / holdout fixture が GT の組み立てに届かない（読んだラベルの件数で証明）/ 除外されるラベルの種類の件数 / 未対応のラベルの意味 → BLOCKED / 1 作品に複数単位 / run が適格でない単位は件数に出る / PR4-1 のセクションは GT の有無で変わらない / `final_promotion` は NOT_RUN / A2 は NOT_APPLICABLE / rank・top-3・候補の参照なし・絶対パス固定なし。合成・一時 DB のみ（本番 DB なし）。

## チェックポイントの記録（PR4-2A が先に commit されたこと）

```
git log -2 --oneline
5241d62 docs: freeze PR4 diagnostic GT provenance bridge
687adb1 docs: freeze PR4 diagnostic GT semantics

git show --stat 5241d62
 docs/PR4_2A_FREEZE.txt                    |  16 ++
 docs/PR4_2A_REPORT.md                     |  48 ++++++
 docs/PR4_DIAGNOSTIC_GT_BRIDGE_CONTRACT.md | 101 ++++++++++++
 docs/pr4_gt_bridge_schema.json            | 255 ++++++++++++++++++++++++++++++
 4 files changed, 420 insertions(+)
```

`5241d62` は PR4-2A の docs 4 ファイルだけを含む commit で、origin と同期済み（PR4-3 の作業は、この commit の上の未 commit の変更）。

## 注意・限界

- **実データでは未検証。** 実 DB の read-only 検証は別 gate。実データでは、DB 側の信頼できる GT の件数（40 のはず）や、D / F の task の DB 上の状態が整合性の検査（44）を通るかが初めて分かる。通らなければ `gt_universe_mismatch` で BLOCKED になる（意図した動作）。
- bridge ファイルには `status` 項目が無いので、「READY」は検証の全てを通ったことを意味する（`bridge_status`）。
- 44 は GT の単位数の整合性の検査で、precision の分母ではない。分母は matcher ごとの AUTO の評価単位（例: rules-safe が 31 件、rules-tags-shadow が 35 件、のような値は正常）。
- rules-safe と rules-tags-shadow は入力の候補集合が違うので、matcher 間の precision の比較はその前提で読む。
- アプリ本体（UI / command）からはまだ呼ばれない。
