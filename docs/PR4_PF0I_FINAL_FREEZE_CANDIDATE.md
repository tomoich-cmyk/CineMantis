# PR4-PF0I FINAL FREEZE CANDIDATE

判定: **PF0I FINAL FREEZE candidate（PASS）。commit / push 未実施（Human Gate 待ち）。** FINAL = NOT_RUN。実 ledger・H2 payload・GT・machine output・holdout・production DB は読んでいない（synthetic のみ）。

## anchor
| 項目 | 値 |
|---|---|
| PF0I base（既 push） | `889962b5235a747a511dc9c88e457dd3e4d45720`（親 `595cd4f`） |
| PF0G authoritative docs / PF0G1 実装 | `1a6266d9869d7dc3bf8556850312c85676ebe7d3` / `eeeb57353b0e610e0827ce4882900c4bd156a22e` |
| PF0G2 | `9fbd64faae611ba57411d0ca2ea4b22d2697024c`（`claude/pr4-pf0g2-210938`） |
| PF0H1 | `81e637210f7324686f72b7f6be850950c2d9e04e`（`claude/pr4-pf0h1-e60774`）。loader 規則 `pr4-pf0h1-final-manifest-sha-1` |
| PF0C policy canonical SHA | `1d575b41f2fd22ebb36b8b90a7a45a8ef3e038906f3f634ecd84c472fb232ac4` |

## 今回の追加
1. **PF0H loader（唯一）** `load_pf0h_final_membership`: `FINAL_MEMBERSHIP_COMMITMENT.json` を JSON parse → canonical JSON → UTF-8 → SHA-256（体裁・CRLF・pretty/compact に依存しない）。`authoritative_membership_commitment` = `membership_commitment_sha256`（`protocol` と `authoritative:true` が必須）。補助値・`eligibility_rule_version` は読まない。`H2_CLOSE.json` の raw SHA は binding に使えない（t37・e3 で拒否を確認）。
2. **PF0G2 ↔ PF0I E2E（統合 worktree・synthetic）**: PF0G2 packager が出した bundle・`build_secret_commitment`・pin・実 `Store` が seal した ledger を adapter が受理（READY）し、件数が PF0G1 の completion audit と一致（confirm / pick_other / none / defer_final / corrections）。PF0H1 の `final_manifest_sha256` と loader の出力が一致。改ざん（bundle・ledger・seal 欠落・各 anchor・raw SHA・別 secret・pre-seal ledger）は全て fail closed。errata は PF0G1 の `evaluation_validity` が `Invalidated` になることだけ確認（判定は FINAL runner の責務）。
3. 契約・DEFERRED_FIXES の更新（D4 / D6 / D1 / D2 クローズ、D8 追加）。

## 統合手順（E2E の再現）
PF0I 単体 branch は PF0G2 / PF0H1 のコードを持たないので、`pr4_pf0i_e2e_tests.rs` は `mod.rs` に登録していない。再現: 一時 worktree で `claude/pr4-pf0i-653127` に `81e6372` と `9fbd64f` を merge（`mod.rs` / `DEFERRED_FIXES.md` の add/add 衝突は両方を残す）→ `pr4_pf0g2_tests.rs` の末尾に `#[path = "pr4_pf0i_e2e_tests.rs"] mod pf0i_e2e;` を追加 → `cargo test --lib pf0i`。main に 3 branch が入った時点で `mod.rs` へ登録する（DEFERRED D8）。

## 試験結果
- PF0I 単体 branch: `cargo test --lib` **618 passed / 0 failed / 12 ignored**（PF0I 37。t38 = 必須 fail-closed マトリクス: wrong PF0H commit anchor・malformed canonical JSON・missing anchor・wrong build_secret_commitment・unexpected bundle。wrong final manifest SHA / membership / pin mismatch / malformed bundle は t09・t14・t06・t15・e3 が既に網羅）。
- 統合 worktree（PF0I + PF0H1 + PF0G2。t38 / `load_pf0h_final_membership_anchored` 追加前の adapter で実行。追加は既存関数を変えない純粋な追加で、統合の再実行はしていない）: `cargo test --lib` **678 passed / 0 failed / 13 ignored**。`pf0i` 絞り込み 41 passed（PF0I 36 + `pf0i_e2e` 4 + PF0G2 の handoff 1）。統合 worktree は検証後に削除済み（commit しない・push しない）。

## ファイル SHA-256（LF 正規化後 = commit される bytes）
| ファイル | SHA-256 |
|---|---|
| services/pr4_pf0i_adapter.rs | 2895beda6b735efabc46e39da43150a34aae03567a47de1cc7d5ade5844bc349 |
| services/pr4_pf0i_tests.rs | c710787b559a467a9e73added5014cd4b7aa1f9c0eca4e5dbb03bc3bf4e84b52 |
| services/pr4_pf0i_e2e_tests.rs（新規・未登録） | efeb7aa10899c06c40e5b996fdfc4fb4240bbbd943a64fa1938298eb766c9557 |
| docs/PR4_PF0I0_GT_EVALUATOR_ADAPTER_CONTRACT.md | df0ce7803777bb79ee6e9208859b6c295e856bcb257d3bf0cd7d6bc124d6fee4 |
| docs/pr4_pf0i0_gt_adapter_schema.json | 69e365f36991eee0754cb5a22202a43923e5d2ae694115b72c4920cbe1434419 |
| DEFERRED_FIXES.md | d374c07c02220c04c20ea066cacbc9fefb6c3cb119aa7abe4ca5be83841834b8 |

## Hard Gate（変更なし）
adapter の評価結果だけでは promotion 不可。FINAL runner が promotion-impacting errata なし（evaluation ≠ INVALIDATED）を確認して初めて authoritative evaluation になる。
