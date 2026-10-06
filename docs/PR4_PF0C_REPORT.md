# PR4-PF0C: binding な統計・promotion policy の凍結 — 報告

状態: **契約とテストを追加して凍結。** holdout は開いていない。H1（25 件・commitment `22cc00a8…e031`）は温存。FINAL は NOT_RUN。policy freeze（全体）は未了。

- 契約: `docs/PR4_PF0C_FINAL_POLICY_CONTRACT.md`、機械可読: `docs/pr4_final_policy.json`（canonical JSON の SHA-256 `1d575b41f2fd22ebb36b8b90a7a45a8ef3e038906f3f634ecd84c472fb232ac4`）。
- 決めた数値（あなたと監査側が、結果を見る前に決めたもの）: 片側 exact Clopper–Pearson・信頼水準 95%・要求する下限 99%・最低 AUTO 件数 299・最低 AUTO coverage 50%。AUTO の GT 未解決は INCONCLUSIVE。abstention と誤 AUTO の件数は必須併記で独立した閾値なし。結果を見た後の標本の追加と、性能を見た後の policy の変更は禁止。
- 帰結: 全正解でも 298 件では 99% に届かず、299 件で届く。299 件の AUTO に誤りが 1 件でもあれば NO PROMOTION（誤りが 1・2・3・4・5 件のときの最小の AUTO 件数は 473・628・773・913・1049）。最低 coverage 50% でも 299 件の AUTO を確保するための eligible な holdout は 598 件。H1 の 25 件を含め、H2 は 573 件以上（最終の eligibility の述語を通った件数）。598 / 573 は蓄積の設計の下限で、評価の条件ではない。

## 変更したファイル
| ファイル | 内容 |
|---|---|
| `apps/desktop/src-tauri/src/services/pr4_pf0b.rs` | `PolicyConfig::from_frozen_json`（未知のキー・不足・型の違い・想定と違う分類を拒否）と、凍結 policy の値どおりに判定することを確かめるテスト 5 件（test ビルドだけ） |
| `docs/pr4_final_policy.json`、`docs/PR4_PF0C_FINAL_POLICY_CONTRACT.md`、`docs/PR4_PF0C_REPORT.md`、`docs/PR4_PF0C_FREEZE.txt`（新規） | policy、契約、この報告、凍結ハッシュ |
| `docs/PR4_PF0B_REPORT.md`、`docs/PR4_PF0B_FREEZE.txt` | 別セッションでの commit についての一文と、凍結ファイルへの追記（元の行は残す） |

製品のコード・migration・DB・matcher / rules / Jev・PR4-0〜PF0B の凍結済みの内容は変えていない。

## テスト
| 実行 | 結果 |
|---|---|
| `cargo test --lib pr4_pf0b` | 25 passed / 0 failed（PF0B の 20 + PF0C の 5） |
| `cargo test --lib`（全件の回帰） | 532 passed / 0 failed / 10 ignored（もともとの手動用 `#[ignore]`） |
| `cargo check --lib` | 通過 |

PF0C の 5 件: 凍結 policy が合意した値と固定した SHA-256 を持つ（H1 の 25 + H2 の 573 = 598）/ 形の逸脱（未知のキー・不足・独立した誤り AUTO の上限・abstention や wrong AUTO の閾値・Exclude / CountAsWrong・方式の違い・事後の標本追加や変更を許す記述・coverage の範囲外など 13 通り）を拒否し、数値を 1 つ変えると provenance の SHA-256 も変わる / 凍結 policy の判定（299 件で PROMOTE・298 件で INCONCLUSIVE・299 件中 1 件の誤りで NO PROMOTION・coverage 不足で NO PROMOTION・ちょうど 50% は満たす・AUTO の GT 未解決で INCONCLUSIVE・技術的な失敗は判定不可・AUTO 0 件）/ 誤りの件数ごとの最小 AUTO 件数の表 / policy の JSON に評価単位の値が無い。

## 次（未着手）
H2 の蓄積の手順の設計（H1 = 25 を永久固定 → H2 を性能非参照で 573 件以上 → 重複なしで union → FINAL の membership の commitment を凍結）→ policy freeze（凍結版で機械出力を 1 回生成する条件を含む）→ sealed な FINAL 評価。
