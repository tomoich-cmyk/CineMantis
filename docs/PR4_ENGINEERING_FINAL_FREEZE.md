# PR4 ENGINEERING FINAL FREEZE（docs-only freeze record）

日付: 2026-10-11 / 種別: docs-only（製品挙動の変更なし・H2 非接触）

## 1. Engineering freeze commit candidate
- **`55537e6d26172cdfbea8b79ec0636cf53a9c3d4f`**（PF0G3, branch `claude/pr4-pf0g3-74a31b`）
- 本 freeze record は上記 commit の上に docs 1 ファイルを足すのみ（source diff = 0）。評価対象の engineering reference は 55537e6 の tree。

## 2. Upstream anchors（全て 55537e6 の祖先であることを `git merge-base --is-ancestor` で確認済み）
| lane | full SHA | branch |
|---|---|---|
| PF0G docs/schema | `1a6266d9869d7dc3bf8556850312c85676ebe7d3` | claude/pr4-pf0g-171907 |
| PF0G2 | `9fbd64faae611ba57411d0ca2ea4b22d2697024c` | claude/pr4-pf0g2-210938 |
| PF0H1 | `81e637210f7324686f72b7f6be850950c2d9e04e` | claude/pr4-pf0h1-e60774 |
| PF0I | `6ddbc5b3b56e30c14877688c23169fd106251848` | claude/pr4-pf0i-653127 |
| G/H/I integration | `954ac81d7c3f8acb45a6d60b162ad876918d3b9a` | claude/pr4-ghi-integration-20261011 |
| PF0G3 | `55537e6d26172cdfbea8b79ec0636cf53a9c3d4f` | claude/pr4-pf0g3-74a31b |

欠落している candidate branch なし。

## 3. Evidence
- `cargo test --lib` = **687 passed / 0 failed / 14 ignored**（docs/PR4_PF0G3_REPORT.md §E。本 closure では再実行していない）
- PF0G2 / PF0H1 / PF0I PASS、e1–e4 PASS、t38 PASS
- PF0G3 browser E2E **85/85 PASS**（Chrome 152.0.7977.130、Windows 11、viewport 375x812、synthetic 6 unit / 127.0.0.1 loopback、JS error 0、CSP violation は意図した 5 probe のみ）。原本: `docs/pf0g3_evidence/browser_e2e_result.json`
- CSP qualified（`unsafe-inline` 無し。`default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'none'; font-src 'none'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'`）
- reviewer binary qualified（env のみ設定・fail closed・本体稼働中は拒否・example なので製品 bundle 非同梱）
- fail-closed 8 条件 traceability 完了（PR4_PF0G3_REPORT.md §D）
- Engineering Complete = YES

### Hash（sha256, CRLF→LF 正規化。55537e6 の blob から算出）
| file | sha256 |
|---|---|
| src/services/pr4_pf0g3_reviewer.rs | 471730d3baf667b8e410fa6b9d233129ef9555bd933900cdd9e6fc42aa7957e6 |
| src/services/pr4_pf0g3_tests.rs | 8392782de3848f8321e068a6b8adf2961ec7d5eb173dd118d49d1dca4dd26232 |
| examples/pf0g3_reviewer.rs | 93bedf136f56bac6725aa980bb537c9a87d6d4c47bcdcf4399124b325f78a604 |
| pr4_pf0g2_ui/index.html | 18c1466fd183c11de75fdde3af967f35fbc553eb686ade13c45290bd59dfc78c |
| pr4_pf0g2_ui/app.js | 1f68ae34857b38b290f4b21003871912327518d64b61f0cdc3db7b56660bb8e5 |
| pr4_pf0g2_ui/app.css | 3c36fc7fcd21710b48beac4fb7b53c188f3e53785a92475f0d570fd0f82c63bd |
| Cargo.toml（src-tauri） | 88e092698d8930cbb6ffa8533f8be4cbc6cbfce7566d1052017f36f237bad3a8 |
| docs/pf0g3_evidence/browser_e2e_result.json（raw） | f037de5b76e650cab814e35631796557e67b32fcf8f9b9e3ec1a3298c953f472 |
| docs/pf0g3_evidence/browser_e2e.js（raw） | c9d6d8e2aacbb23de056ea604a5584bf49884dd884e29204a68925161f7e899e |

（パスは `apps/desktop/src-tauri/` 相対。fixture manifest_sha256 = `4a41e932b572bb36aa55a3f76ce654ec0de0653c2e04cabe970f977089cd3a10`、ui_package_sha256 = `82d4ef450cabbe4d367a192993b396d08f758bcab15b4a208a2a5f0b6b535f1c`。PF0G2 時点の source hash は docs/PR4_PF0G2_FREEZE.txt を参照。）

## 4. Closed items（再オープンしない）
- D5 = N/A（distinct commitment domains）
- D8 = accepted（`#[path]` child-module registration）
- M1 = seeded shuffle 実装済み
- M2 = `build_secret_commitment` semantic verification 実装済み
- PF0G3 browser qualification / CSP / reviewer runtime・binary

## 5. Accepted LOW / DEFERRED（block しない。`DEFERRED_FIXES.md`）
- G3-1: seal 後の `/api/next` は 410 でなく 200 `{done:true}`
- G3-2: Chrome 152 のみ検証。screen reader は独立検証なし
- G3-3: reviewer 起動 runbook 未作成
- G3-4: `/app.js` `/app.css` は token なしで配信（secret を含まず pin 済み）

追加の受容事項:
- FINAL runner は promotion に影響する errata を必ず確認する
- adapter result 単独では promotion を承認しない
- external anchor の exact-match は FINAL preflight の責務
- seeded shuffle の具体 algorithm（SHA256(seed|cand|token)）は実装上の選択（contract §3.2 は規定していない）

## 6. Freeze scope（重要）
本 freeze は **engineering 実装の凍結のみ**。以下を意味しない:
- H2 qualification 完了ではない（H2 target = 946、現在 0）
- FINAL PASS ではない（FINAL = NOT_RUN）
- production promotion 承認ではない
- main / feat/awards-festivals への統合承認ではない

H2 期間中は engineering reference を変更しない。bug fix は別 gate「freeze amendment」とする。

## 7. 禁止される再解釈
- 本 record を H2/FINAL の合格・昇格根拠として引用しない
- 受容 LOW を「解決済み」と記述しない
- 55537e6 以外の tree を engineering reference として評価しない
- 85/85 browser E2E を実データ・本番 TMDB・screen reader での検証と読み替えない
