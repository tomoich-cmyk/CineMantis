# PR4-PF0H1 → PF0I handoff（freeze candidate）

## PF0I が exact-match する値の取り出し元（PF0G R3）
| PF0G の名称 | PF0H の出所 | 性質 |
|---|---|---|
| `authoritative_membership_commitment` | `FINAL_MEMBERSHIP_COMMITMENT.json` の `membership_commitment_sha256`（`authoritative:true`）。PF0B `membership_commitment(keys, version)` の出力そのもの。補助値 `auxiliary_audit_values.*` は exact-match に使わない | 決定的 |
| PF0H final manifest SHA | **canonical SHA-256(`FINAL_MEMBERSHIP_COMMITMENT.json`)**（Human 裁定で確定）。規則 `pr4-pf0h1-final-manifest-sha-1`: JSON を parse → canon（キー昇順・空白なし・整数のみ・文字列は serde_json 形式）→ UTF-8 bytes の SHA-256。実装 `pr4_pf0h_close::final_manifest_sha256`。体裁（pretty/compact/CRLF）に依存しない。`H2_CLOSE.json` の raw SHA は補助証拠で binding anchor ではない | 決定的 |

## 凍結候補
- close/verify 実装 head: 本 branch（commit 前）。内容 SHA は Human Gate の commit 時に確定。
- manifest schema `docs/pr4_pf0h0_close_manifest_schema.json` SHA-256: `e5c45908dd92fc4381617c5c17c8ed6851e2d214e327b9e059b9c1f3e0b61fa9`
- PF0G authoritative: `1a6266d9869d7dc3bf8556850312c85676ebe7d3`（参照のみ）

## 解決済み
「PF0H final manifest SHA」の対象は上記で確定。DEFERRED_FIXES.md の該当項目はクローズ。テスト `the_final_manifest_sha_is_the_canonical_sha_and_ignores_formatting`（golden 値・体裁非依存・2 回 close で一致）。
