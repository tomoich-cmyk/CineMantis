# PR4-PF0I FREEZE CANDIDATE（evidence・provenance）

判定: **freeze candidate。commit / push 未実施（Human Gate 待ち）。** branch `claude/pr4-pf0i-653127`、HEAD = `1a6266d`（0aca746 の子孫。PF0G の docs-aligned authoritative commit へ **fast-forward のみ**。履歴の書換えなし。この branch 独自の commit は無い）。

## 整合した外部の固定点
- PF0G authoritative docs/schema: `1a6266d9869d7dc3bf8556850312c85676ebe7d3` / PF0G1 実装 freeze: `eeeb57353b0e610e0827ce4882900c4bd156a22e`
- PF0C policy canonical SHA: `1d575b41f2fd22ebb36b8b90a7a45a8ef3e038906f3f634ecd84c472fb232ac4`（`docs/pr4_final_policy.json` を `PolicyConfig::from_frozen_json` で読み、t24 で一致を確認）
- PF0H1: `claude/pr4-pf0h1-e60774` は未 commit の freeze candidate（READ ONLY で handoff 文書だけ参照）。loader は未実装（DEFERRED D4）

## ファイル SHA-256（作業ツリー）
| ファイル | SHA-256 |
|---|---|
| docs/PR4_PF0I0_GT_EVALUATOR_ADAPTER_CONTRACT.md | 715abbae35d69c003d7190112301534961db168b8cbd9c6c8db28a0298d8152d |
| docs/pr4_pf0i0_gt_adapter_schema.json | 270f351ec463e6b02cb5cab19d9129ceab563f372c3fcf53231fb48fd723e07c |
| apps/desktop/src-tauri/src/services/pr4_pf0i_adapter.rs（新規） | 2a0e0e7a13f07ccbc77adec76f287ed065271411b7189342de37ebc7943438ed |
| apps/desktop/src-tauri/src/services/pr4_pf0i_tests.rs（新規） | 8c77e691b52eeb28ef22872c5afab515e496637b80b9044540b4d118853873ed |
| apps/desktop/src-tauri/src/services/mod.rs（+3 行・`#[cfg(test)] pub mod pr4_pf0i_adapter;`） | 5c7880fef0a0939331030630aea33421c37bc1e6b7076c2ac5df6462458ee420 |
| DEFERRED_FIXES.md（新規） | 2e334308f896eb76204734b9757dbe90e97c2de988b73b34840a233f4b30a305 |
| （無変更の参照）pr4_pf0b.rs | acf2e8daed8947c45e5072e72f4c3cc190333e042877c7be06fe36451b83c850 |
| （無変更の参照）pr4_blind_gt.rs | 666182cb150d6a7c7605268f286cb726858e23e72eaa646fd3b3afd9f6d3e3cc |
| （無変更の参照）docs/pr4_final_policy.json | 8f38cdd21f28f196ff7bcd12e59e4495d8507ecb0c794ccb9e408f1d042b2170 |

## 試験
- `cargo test --lib pr4_pf0i`: 33 passed / 0 failed。`cargo test --lib`（全体）: 613 passed / 0 failed / 12 ignored（PF0G1 までの 581 + PF0I 32 + t34 の schema 整合 1 − 重複なし。base 比で既存試験の失敗なし）。
- `pr4_pf0b.rs` の差分 0（`git diff` 空）。

## 今回触れていないもの
実 ledger・payload・production DB・H2・H1・sealed holdout・main H2 の daily runner・主 checkout（`C:\Users\tyama\CineMantis` は `feat/awards-festivals` のまま未接触）。
