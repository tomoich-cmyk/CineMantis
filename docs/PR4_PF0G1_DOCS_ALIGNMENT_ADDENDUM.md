# PR4-PF0G1 Docs Alignment Addendum

本書は docs-only の整合性訂正の記録である。実装は変更していない。

## 経緯
- PF0G1 implementation freeze commit = `eeeb57353b0e610e0827ce4882900c4bd156a22e`（branch `claude/pr4-pf0g-171907`）。
- そこで凍結された契約（`docs/PR4_PF0G0_BLIND_GT_CONTRACT.md`）と schema（`docs/pr4_pf0g0_blind_gt_schema.json`）には、監査差し戻し（D7 / R1〜R4 / U3）を §17 として追記する前の旧記述が残っていた。§17 は §15・§16 の旧記述に優先すると書かれていたが、正本自身が自己矛盾した状態だった。

## この訂正が行ったこと（docs のみ）
- 契約 §16 の 1 の旧 candidate_token 定義（`H(unit_salt | media_type | tmdb_id)` と、`unit_salt` から token を計算して重複を照合する方式）を **obsolete** と明記し、現行の正本を §17 R4（`HMAC-SHA256(build_secret, domain + review_unit_id + media_type + tmdb_id)`、重複照合は `overlap_digests`）に一本化した。
- 契約から `candidate_ref` と `"c1"` / `"c2"` の旧例を除去し、confirm の ledger event は opaque な `candidate_token` のみを記録する記述に統一した。
- 契約の status 行と schema JSON の `status`（`DRAFT_FOR_REVIEW`）を、PF0G1 で凍結済みの状態に更新した。
- D7（`build_secret` / `unblind_map` の隔離）、A8 leak scan の seal 必須入力、seal event の `event_commitment` = review seal SHA、seal 後の GT 不変と promotion-impacting errata = INVALIDATED の記述は変更していない。

## 変更していないもの
- `services/pr4_blind_gt.rs`、`services/pr4_blind_gt_tests.rs`、その他すべての source（source diff = 0）。実装・テスト・token・pin・seal・errata・reviewer package のロジックは `eeeb573` のまま。
- `docs/PR4_PF0G1_FREEZE.txt` は `eeeb573` 時点の historical freeze record として**不変**（byte 一致）。そこに記録された `pr4_blind_gt.rs` の SHA-256（`666182cb150d6a7c7605268f286cb726858e23e72eaa646fd3b3afd9f6d3e3cc`）は、今も実装の SHA である。FREEZE.txt に記録された契約・schema の SHA-256 は `eeeb573` 時点のものであり、本訂正で変わった。

## authoritative な参照先
- PF0G1 implementation freeze = `eeeb57353b0e610e0827ce4882900c4bd156a22e`（実装・テストの正本）。
- PF0G docs-aligned authoritative schema / contract = **本書を含む後続（successor）の docs-only commit**。PF0G2 は、その successor commit の SHA を authoritative schema SHA として使う。契約・schema の現行 SHA-256 は、その commit の tree から取る。
