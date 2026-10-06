# PR4-PF0C: FINAL の binding な統計・promotion policy（凍結）

状態: **policy の数値を契約として凍結。** holdout は開いていない。H1（25 件）は温存。FINAL は NOT_RUN。policy freeze（全体）は未了。
機械可読版: `docs/pr4_final_policy.json`（`policy_version = pr4-final-policy-1`）。
canonical JSON の SHA-256（キーを辞書順に並べ、区切りなしの compact、UTF-8）: `1d575b41f2fd22ebb36b8b90a7a45a8ef3e038906f3f634ecd84c472fb232ac4`。再計算: `json.dumps(obj, sort_keys=True, separators=(',', ':'), ensure_ascii=False)` の SHA-256。この値は、予行演習のコード（`PolicyConfig::from_frozen_json`）が `Frozen` の provenance として使い、テストで固定している。

## 1. binding policy

| 項目 | 値 |
|---|---|
| 信頼区間 | 片側の exact Clopper–Pearson |
| 信頼水準 | 95% |
| 要求する precision の下限 | **99%** |
| 最低 AUTO 件数 | **299** |
| 最低 AUTO coverage | **50%**（判定条件） |
| AUTO した単位の GT が未解決（defer / unresolved） | **INCONCLUSIVE**（`BlockEvaluation`） |
| abstention rate | 必須併記・独立した閾値なし（`coverage + abstention = 1` なので二重にしない） |
| 誤 AUTO の件数 | 必須併記・独立した閾値なし |
| AUTO の件数が不足 | INCONCLUSIVE（閾値は緩めない） |
| coverage < 50% | NO PROMOTION |
| 下限 < 99% | NO PROMOTION |
| 技術的な失敗 | TECHNICAL FAILURE（判定不可。性能の指標を出さない） |
| 結果を見た後の標本の追加 | **禁止** |
| holdout の性能を見た後の policy の変更（coverage を 40% に下げる・下限を 98% に下げる・最低 AUTO 件数を減らす・H2 を途中で足す など） | **禁止** |

- 判定の優先順位: 技術的な失敗 → INCONCLUSIVE（AUTO 不足 / AUTO の GT 未解決）→ NO PROMOTION（coverage・下限の未達）→ PROMOTE。
- 誤 AUTO の件数に別の「0 件必須」を置かない理由: 99% の下限の条件が、誤りの件数を統計的に自然に制約する。別に 0 件の上限を置くと、標本が増えるほど必要以上に厳しくなる。
- 最低 coverage 50% の根拠（製品の要件）: 「少なくとも半分を安全に自動化できること」。これを下回ると、高精度でも大半を人手に回す構成になる。標本数を減らすために coverage を高く（または低く）設定する、という逆算はしていない。

## 2. 数値の帰結（独立に scipy でも照合した値）

99% の下限（片側 95%）を満たすのに要る AUTO 件数の最小値:

| AUTO 中の誤り | 0 | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|---|
| 最小の AUTO 件数 | **299** | 473 | 628 | 773 | 913 | 1049 |

- 全正解でも、298 件では 99% に届かない（下限 0.98999…）。299 件で 0.99003。**299 件の AUTO に誤りが 1 件でもあれば、NO PROMOTION。**
- 最低の標本規模: coverage が最低許容値 50% ちょうどでも 299 件の AUTO を確保するには、eligible な holdout が **ceil(299 / 0.5) = 598 件**。H1 の 25 件（commitment `22cc00a81f3bf58049c1aaf45668e4fdf7b678662cf9c99cf2b591a8f983e031`）を含めて、H2 は少なくとも **573 件（最終の eligibility の述語を通った件数）**。
- 598 / 573 は、蓄積の**設計の下限**であって、評価の条件ではない（評価の条件は上の表）。coverage が 50% を超えれば、AUTO は 299 より多くなりうる。H2 の収集中に strong ラベルなどで除外されるものが出れば、その分、source をさらに積む。

## 3. 凍結されるもの・されないもの
- 凍結: 上の表のすべて。以後、変更は不可。
- まだ凍結していない（次の gate）: H2 の蓄積の手順（性能を見ずに 573 件以上を積む・H1 と重複なしで union・FINAL の membership の commitment を凍結）/ 凍結版の機械出力の生成（候補生成の実行経路・検索のパラメータ・TMDB の request の意味・同点処理・決定ポリシー・閾値）/ policy freeze 全体 / 凍結した機械出力から FINAL の入力への変換層。

## 4. 検証
`pr4_pf0b.rs` のテストが、この JSON から `PolicyConfig` を作り（未知のキー・不足・型の違い・想定と違う分類は拒否）、次を確認する: 値どおりの設定 / canonical SHA-256 の固定 / 299 件の AUTO・全正解・coverage 50% で PROMOTE / 298 件で INCONCLUSIVE / 299 件中 1 件の誤りで NO PROMOTION / coverage 未満で NO PROMOTION / ちょうど 50% は満たす / AUTO の GT 未解決で INCONCLUSIVE / 技術的な失敗は判定不可 / AUTO 0 件 / 上の表の最小件数。
