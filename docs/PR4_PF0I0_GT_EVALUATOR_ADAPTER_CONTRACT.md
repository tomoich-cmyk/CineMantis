# PR4-PF0I0: Frozen Blind-GT → FINAL Evaluator Adapter 契約（r2・freeze candidate）

状態: **r2 freeze candidate。commit / push 前（Human Gate 待ち）。** 実装は `services/pr4_pf0i_adapter.rs`（PF0I1・synthetic のみ・test ビルドだけ）。production label へは書かない。H2 実データ・H1・holdout・production DB は読んでいない。FINAL は NOT_RUN。
機械可読 schema: `docs/pr4_pf0i0_gt_adapter_schema.json`（`adapter_version = pr4-pf0i0-gt-adapter-1`）。

## 0. 整合の基準（1 回だけ行った final alignment）

| 対象 | 値 |
|---|---|
| PF0G authoritative docs/schema | `1a6266d9869d7dc3bf8556850312c85676ebe7d3`（`claude/pr4-pf0g-171907`） |
| PF0G1 実装 freeze | `eeeb57353b0e610e0827ce4882900c4bd156a22e`（source diff は 1a6266d まで 0） |
| `pr4_blind_gt.rs` SHA-256 | `666182cb150d6a7c7605268f286cb726858e23e72eaa646fd3b3afd9f6d3e3cc`（schema の alignment と test t34 で固定） |
| PF0C policy canonical SHA | `1d575b41f2fd22ebb36b8b90a7a45a8ef3e038906f3f634ecd84c472fb232ac4` |
| PF0H | `claude/pr4-pf0h1-e60774` は freeze candidate・未 commit。**PF0H の正式 loader は PF0H schema 確定後に 1 つだけ**実装する（本 gate では未実装。adapter は loader の 2 値を受けるだけ。§7） |

r1 → r2 で PF0G の実形式に合わせた主な変更: pins は init/seal の `pins` object（8 項目）/ confirm event は candidate_token のみ（identity inline は STOP）/ candidate_token = `ct_`+20 文字の HMAC / unblind bundle の形を PF0I が定義 / 失敗は独自 envelope / PF0B は変更しない。

## 1. 不変条件

```
PF0C policy は binding・不変（95% 片側 exact CP / 下限 99% / 最低 AUTO 299 / coverage 50% / 未解決 GT × AUTO = BlockEvaluation）。
PF0B（pr4_pf0b.rs）は一切変更しない。adapter は evaluate() の前段の変換層で、判定ロジックを持たない。
membership の authority は PF0B の membership_commitment だけ。adapter は呼ぶだけで再実装しない。
```
- adapter は **GT 側だけ**。機械出力は assembler が key で結合する。**fail closed**（曖昧・矛盾・欠落は REFUSED）。**何も書き戻さない**（永続化してよいのは count-only report だけ）。
- `trusted_gt_source = frozen_blind_h2_gt` は adapter の全検査を通した `TrustedGtSet` にだけ付く。DB の GT・bridge・H1 の GT は混ぜない・補完しない。
- adapter は file・DB・network・環境変数に触れない（入力は bytes と値だけ。静的検査 t29）。

## 2. 入力

| 入力 | 所有 | 内容 |
|---|---|---|
| ledger bytes | PF0G | `events.jsonl`。グローバル 1 本の hash chain。各行は `canon(行)` と一致 |
| unblind bundle bytes | PF0G2 packager が出し、PF0I が形を定義 | `build_secret_hex` と `review_unit_id → work_key / candidates(token → media_type, tmdb_id)`。**reviewer 側から完全に隔離**。review seal 後にだけ adapter が受け取る。review 開始前に渡るのは `build_secret_commitment` と `unblind_map_sha256` だけ |
| `Anchors` | FINAL runner（audit anchor chain） | `expected_membership_commitment` / `expected_pf0h_final_manifest_sha256` / `review_seal_sha256`（seal event の `event_commitment`）/ `machine_freeze_anchor_commitment` / `eligibility_rule_version_opaque` |
| `Pf0hLoaded` | PF0H の正式 loader | `authoritative_membership_commitment` / `final_manifest_sha256` |
| `PolicyConfig` | PF0C | 凍結 policy（`Frozen{1d575b41…}`・`BlockEvaluation`）以外は REFUSE |

anchors / loader の値が hex64 でない・version が空 → `anchor_missing`。seal / ledger の自己申告だけは信じない。

## 3. canonical GT と写像

出力単位は `(key, gt)` のみ。`key = work:<base10 integer>`（先頭ゼロ・符号・空白・全角数字禁止。`validate_unit_key`。reviewer には見せない）。

| 有効な終端判断 | 条件 | PF0B `Gt` |
|---|---|---|
| `decision`/`correction` の `confirm` | `candidate_token` が bundle のその unit の候補集合に属する。`tmdb_id`/`media_type`/`candidate_ref`/`tmdb_verification` が在れば `inline_identity_present` | `Positive`（identity = bundle の写像。ledger には identity が無い） |
| 〃 `pick_other` | identity 有効・`tmdb_verification` 最小再確認（200・id/media_type 一致・body SHA hex64）・**候補集合に含まれない**・candidate_token は null | `Positive`（identity = event） |
| 〃 `none` | identity・token すべて null | `NoMatch` |
| `defer_final` | — | `Unresolved` |
| 1 巡目 `defer` で止まる | — | REFUSE `non_terminal_state_in_sealed_ledger` |
| member に outcome 無し | — | REFUSE `gt_missing_for_member`（`Unresolved` に丸めない） |
| bundle に無い unit の outcome | — | REFUSE `review_unit_without_map_entry` |
| `qc_decision` | **GT に使わない**。終端状態の unit にしか現れない | — |

`none` の search 証跡（PF0G の A5）と QC の独立性は PF0G の audit の責務で、adapter は再検査しない。自由記述（`note` / `query` / `reviewer_id` / `defer_reason` / `title` 等）は読まない・複製しない。

## 4. bundle・token・secret の検証

1. bundle の**実 bytes の SHA-256 = pin `unblind_map_sha256`** を **parse の前**に照合（壊れた bytes は SHA 不一致が先）。
2. 形: `bundle_version = pr4-pf0i-unblind-bundle-1`、未知キー禁止、unit ≥ 1。自己 hash 項目は持たない。
3. `build_secret_commitment = SHA256(build_secret_hex の ASCII bytes)` を pin と照合（**PF0G は値の定義を持たないので PF0I が固定**。PF0G2 packager が従う。§10 の判断事項 1）。
4. 全単射: `review_unit_id` 重複なし・`work_key` 重複なし・unit 内で token・identity 重複なし。
5. **再導出**: 全 unit で `derive_review_unit_id(build_secret, work_key) == review_unit_id`、全 candidate で `derive_candidate_token(build_secret, review_unit_id, media_type, tmdb_id) == candidate_token`（`candidate_token = HMAC-SHA256(build_secret, "pr4-pf0g-candidate-token-1" \n review_unit_id \n media_type \n tmdb_id)`）。map の付け替え・identity の書換えを検出する。PF0G1 の pub fn を呼ぶ（再実装しない）。
6. ledger に現れる outcome の unit は全て bundle にある。bundle の全 unit に終端判断がある。
7. join は adapter 内部のみ。`review_unit_id`・token は DTO・report・エラーに出さない。

## 5. replay（終端判断の取り出しだけ。PF0G1 の状態機械は再実装しない）

unit ごとの outcome event 列: `decision | defer→(decision | defer_final)` の後に 0 回以上の `correction`。有効 GT = 最後の {decision, correction, defer_final}。

| 列 | 扱い |
|---|---|
| `decision` が 2 つ以上 | 同一内容 `duplicate_record_identical`／違えば `gt_conflict`（「最新が勝つ」「confirm 優先」はしない） |
| `decision` の後の `defer`、`defer_final` の後の `decision`、`defer` の連続 | `gt_conflict` |
| `correction` の reason が 4 値の外 | `correction_reason_invalid` |
| `correction` が終端状態の前 | `correction_chain_invalid` |
| `submission_id` が outcome event 間で重複 | `gt_conflict` |

seal 後の GT 変更は一切許さない（誤りは errata のみ。promotion-impacting なら evaluation = INVALIDATED で新 cycle が必要。**adapter は errata.jsonl を読まない** — §10 の判断事項 3）。`n_corrections_applied`（件数のみ）を report に出す。

## 6. unresolved GT × AUTO

adapter は規則を**実装しない**。(1) `defer_final` を `Gt::Unresolved` にそのまま写す（NoMatch への格下げ・補完・自動 resolve をしない）。(2) policy が凍結 policy（`Frozen{1d575b41…}`）かつ `BlockEvaluation` であることを確認（それ以外は REFUSE `policy_not_pinned_frozen`）。(3) AUTO × Unresolved は PF0B が `Inconclusive(["auto_with_unresolved_gt"])`・`precision_binding=false`。AUTO でない単位の Unresolved は影響しない（t21 / t23 mutation で固定）。

## 7. 検証順序（前段が失敗したら後段へ進まない）

| # | 検査 | 失敗コード |
|---|---|---|
| 1 | anchors / loader 値が揃っている | `anchor_missing` |
| 2 | policy が凍結 policy | `policy_not_pinned_frozen` |
| 3 | ledger の chain: 各行 canonical・`seq` 0 起点欠番なし・`prev_commitment` 連鎖・`event_commitment` 再計算 | `ledger_chain_broken` / `ledger_seq_gap` |
| 4 | 未知の event type なし・`init` が先頭に 1 つ・`seal` が 1 つで最後 | `unknown_event_type` / `init_missing_or_not_first` / `seal_missing_or_duplicated` / `events_after_seal` |
| 5 | **seal の `event_commitment` = anchor `review_seal_sha256`** | `review_seal_sha_mismatch` |
| 6 | seal の `ledger_head_before` = 直前 event、`audit_report_sha256` が hex64（audit は再実行しない） | `ledger_head_before_mismatch` / `seal_pins_missing` |
| 7 | **8 pin が init と seal で完全一致**（キー集合は厳密に 8・hex64） | `seal_pins_missing` / `seal_pins_differ_from_init` |
| 8 | `integrity_anomaly` が 0 件 | `integrity_anomaly_present` |
| 9 | pin `machine_freeze_anchor_commitment` = anchor | `machine_freeze_commitment_mismatch` |
| 10 | ledger の構造化項目に機械状態・work key なし（§8） | `ledger_contains_machine_state` / `ledger_contains_work_key` |
| 11 | bundle（§4）、init/seal の `unit_count` = bundle の unit 数 | `unblind_bundle_*` / `build_secret_commitment_mismatch` / `*_not_derivable` / `unit_count_mismatch` |
| 12 | **membership**: bundle の work_key 集合 → PF0B `membership_commitment(keys, version_opaque)` が anchor **と** loader の `authoritative_membership_commitment` の両方に完全一致 | `membership_commitment_mismatch` |
| 13 | **PF0H final manifest SHA**: loader 値 = anchor | `pf0h_final_manifest_mismatch` |
| 14 | replay（§5）と写像（§3） | 各コード |

- **PF0I が PF0H に対して exact-match するのは** authoritative_membership_commitment / PF0H final manifest SHA / review seal SHA（seal event の `event_commitment`）**だけ**。PF0H 内部の version や補助 hash は解釈・分解・比較しない（version は不透明な文字列として `membership_commitment` に渡すだけ）。
- **U8 の順序**（primary → QC → correction → audit PASS → seal → 以後変更禁止 → map 使用 → adapter）の機械検査: seal が最後（4）／seal の存在 = audit PASS（PF0G1 は audit 不合格の seal を拒否。adapter は `audit_report_sha256` の存在のみ）／seal 後に event なし（4）／QC・correction は seal より前（4 の帰結）／bundle の SHA が **init 時点で pin**（seal 後に作った map を使えない。7・11）／外部 anchor の seal SHA が無ければ起動しない（1・5）。「map を seal 前に開かない」ことは PF0G の `unblind_allowed()` と ACL の責務で、adapter から証明できない。
- evaluator 本体も `expected_membership_commitment` で再検査する（二重に守る）。

## 8. identity leakage 防止

(A) GT → 機械/tuning への逆流: 出力は DTO（プロセス内）と count-only report のみ。key 付き GT 一覧・identity・token・`review_unit_id` をファイル/ログ/エラーに出さない（エラーは固定コード。t28 で fixture 実値の不在を assert）。GT 集合は FINAL evaluator 以外に使わない。adapter は DB・network・env・file を持たない。
(B) 機械状態 → reviewer への流入の最低線: ledger の構造化項目（キー名と文字列値。自由記述と `pins` を除く）に機械状態・work key が在れば REFUSE。禁止キーは PF0G1 `scan_forbidden` の KEYS から ledger の正規フィールド `decision` / `tmdbid` を除いたもの（PF0G1 差分 D10）。`pins` のキー名は `unblind` / `buildsecret` を含むので走査から外し、キー集合と値の形を別に厳密検査する。blind の**証明**ではない（UI が何を見せたかは PF0G の責務で、`a8_leak_scan_report_sha256` / `ui_package_sha256` / `shuffle_seed_commitment` の pin 一致だけ検査）。

## 9. 出力・assembler・失敗 envelope

- `TrustedGtSet`（private field・構築は `adapt` の 1 箇所だけ。t32）: `membership_commitment` と `entries: [(work:<id>, Positive|NoMatch|Unresolved)]`（key 昇順）。reviewer・時刻・理由・候補・token・自由記述・SHA 群を持たない。
- `assemble_final_input(machine, &TrustedGtSet, version_opaque) → FinalInput`: units = member key 昇順、`machine.get(key)` が無ければ `Machine::Missing`（PF0B が TECHNICAL_FAILURE）。machine 側に member 外の key があれば `machine_output_for_non_member`。
- **adapter 失敗時は PF0B を呼ばない。** 独自 envelope `{stage:"gt_adapter", status:"TECHNICAL_FAILURE", adapter_version, reason_code, reason_codes}` を返す。**PF0B の report に似せない**（`report_version` / `decision` / `metrics` / `policy_provenance` / `decision_reasons` を持たない。t25 でキー集合が重ならないことを固定）。assembler 失敗も同じ形。
- **PF0B にヘルパを足さない。** `pr4_pf0b.rs` は無変更（git diff で確認。t29 は adapter が PF0B の既存 pub API だけを使うことを固定）。
- adapter report（count-only）: `status`・各 anchor/pin の `match|mismatch|not_checked`・`n_members / n_events / n_corrections_applied / n_confirm / n_pick_other / n_none / n_defer_final`・`refusal_codes`。REFUSED のとき件数は全て 0。

## 10. Human 決定（承認済み）と残る宿題

| # | 決定 / 宿題 | 状態 |
|---|---|---|
| 1 | **`build_secret_commitment = SHA256(ASCII(lowercase_hex(build_secret_bytes)))`**（`0x` なし・空白なし。32 byte の secret なら 64 文字 hex）。PF0G2 packager が従う | 承認済み |
| 2 | **unblind bundle format = `pr4-pf0i-unblind-bundle-1`** を正式な handoff format とする。PF0G2 が生成し、PF0I が strict validation | 承認済み |
| 3 | **promotion-impacting errata の確認は FINAL runner の責務**（adapter に重複実装しない）。**adapter の評価結果だけでは promotion 不可。** FINAL runner が promotion-impacting errata なし（evaluation ≠ INVALIDATED）を確認して初めて authoritative evaluation になる | 承認済み |
| 4 | **PF0H final manifest SHA = `FINAL_MEMBERSHIP_COMMITMENT.json` の canonical SHA-256**。binding anchors は (1) authoritative membership commitment (2) その canonical SHA (3) review seal SHA。`H2_CLOSE.json` の SHA は supplemental evidence であって binding anchor ではない | 確定（D4 クローズ）。loader の本実装は PF0H1 の commit SHA 確定後 |
| 5 | `machine_freeze_anchor_commitment`（PF0G pin）と manifest の `machine_freeze_commitment_sha256` の関係 | LOW・DEFERRED（PF0G2） |
| 6 | 実 `Store` ledger との E2E（fixture は PF0G1 のコードから転記した合成） | DEFERRED（実データ読込は Hard Gate） |

状態は「PF0I implementation PASS（candidate）」で、PF0I FINAL FREEZE は PF0H1 の formalization 待ち。

## 11. 非目標・禁止

production label/DB へ書かない・開発用 GT / H1 GT を混ぜない・adapter で precision/coverage/判定を計算しない・seal 後の GT 修正や record 追加をしない・複数 reviewer の不一致を adapter で解決しない・H2 実データをこの gate で開かない。

## 12. 試験（PF0I1・33 件・全て合成・DB/FS/network なし）

A 写像（t01–t04）/ B join・membership（t05–t10）/ C SHA・seal・順序・改ざん（t11–t15）/ D 複製・衝突・訂正（t16–t18）/ E 評価結合（t20–t26: frozen policy で PROMOTE・AUTO×defer_final で INCONCLUSIVE・none/pick_other で誤り・NoMatch への格下げ mutation で差が出る・非凍結 policy 拒否・独自 envelope・machine 欠落は PF0B の TECHNICAL_FAILURE）/ F leakage・静的性質（t27–t33）/ schema と実装の整合（t34）。
受け入れ: `cargo test --lib` 全 green（本 gate: 613 passed / 0 failed）・PF0B/PF0G1 の既存試験 green・H2 実データ未使用・main checkout 無変更。
