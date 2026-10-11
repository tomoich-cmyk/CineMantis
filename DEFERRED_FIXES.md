# DEFERRED_FIXES（PF0I fast-track）

停止理由にしない LOW / 文書・後続 gate の宿題。

| # | 内容 | 持ち越し先 |
|---|---|---|
| D1 | `build_secret_commitment` の導出式を PF0G は定義していない。PF0I は SHA256(build_secret_hex の ASCII) に固定。PF0G2 packager が従う | PF0G2 |
| D2 | unblind bundle の形（`pr4-pf0i-unblind-bundle-1`）は PF0I 定義。PF0G2 が出力する | PF0G2 |
| D3 | `errata.jsonl` の promotion-impacting（evaluation = INVALIDATED）の確認は FINAL runner の責務（adapter は読まない） | PF0I2 / FINAL runner |
| D4 | **確定**: PF0H final manifest SHA = FINAL_MEMBERSHIP_COMMITMENT.json の canonical SHA-256（H2_CLOSE.json の生 SHA は supplemental evidence・binding anchor ではない）。**loader 実装済み**（`load_pf0h_final_membership`） | PF0I loader gate |
| D5 | PF0G pin `machine_freeze_anchor_commitment` と manifest の `machine_freeze_commitment_sha256` の関係（別値の可能性） | PF0G2 |
| D6 | 実 `Store` が出した ledger との E2E 1 本（PF0G1 の test fixture は private） | PF0G2 完成後 |
| D7 | `pr4_blind_gt` は PF0G1 で `pub mod`（製品ビルドに含まれる）。PF0I adapter は cfg(test) のみ。PF0G 側の判断 | PF0G |
| D8 | PF0G2 packager が出す bundle / build_secret_commitment は統合 E2E で PF0I と一致を確認済み（D1・D2・D6 クローズ）。残: 統合 E2E ファイル `pr4_pf0i_e2e_tests.rs` は単体 branch でコンパイルしないので mod.rs 未登録。3 branch（PF0G2・PF0H1・PF0I）が main に入った後に登録する | main 統合時 |
