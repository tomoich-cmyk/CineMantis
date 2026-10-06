# PR4-PF0B: FINAL パイプラインの予行演習 — 報告

状態: **実装・テスト済み（synthetic のみ）。** holdout は開いていない。FINAL は NOT_RUN。policy freeze は未定義。H1（PF0A の 25 件、commitment `22cc00a81f3bf58049c1aaf45668e4fdf7b678662cf9c99cf2b591a8f983e031`）は温存し、単独では promotion の判定に使わない。
場所: `apps/desktop/src-tauri/src/services/pr4_pf0b.rs`（`#[cfg(test)]`・test ビルドだけ・製品には出ない）。DB・ファイル・通信・環境変数には触れない（ソースを検査するテストあり）。

## 何を確かめるか
「この閾値で promotion できるか」ではなく、**最終評価のコードが、どんな policy を与えても、正しい分母・判定・失敗の扱いで動く**こと。実データでの性能は、holdout まで未知のまま。
流れ: synthetic の評価単位 + 凍結 membership の commitment → membership の検査 → 機械出力の検査 → FINAL レポート（件数・率・信頼区間の下限）→ promotion 規則の判定（`Promote` / `NoPromotion` / `Inconclusive` / `TechnicalFailure`）。

## 定義（明記）
- **信頼区間**: 片側の exact Clopper–Pearson 下限。`successes = k`、`n`、信頼水準 `c` に対し、`P(X >= k | n, p) = 1 - c` を満たす `p` を二分法で求める。`k = 0` → 0、`k = n` → `(1 - c)^(1/n)`。`n = 0`・`k > n`・`c ∉ (0, 1)` は計算しない。
  確認した境界値（c = 0.95）: n = 25 全正解 → 約 88.7% / 全正解で下限 95% 以上になる最小件数は 59 / 99% 以上は 299。
- **precision**: 正しい AUTO ÷ 分母（`(media_type, tmdb_id)` の完全一致。none の GT に AUTO が候補を選べば誤り）。
- **分母 0**: precision と下限は **null**（NaN・無限大・0 にしない）。AUTO が 0 件のときの結論は INCONCLUSIVE。
- **技術的な失敗のとき、性能の指標を出さない**: membership の不一致（追加・削除・重複・版名の違い）、機械出力の欠落、無効な AUTO の identity、実行の失敗、不正な policy、空の membership。この場合、`metrics` は null で、件数も precision も出さない（分母を黙って減らして計算可能にしない）。
- **policy は test-only で非拘束**: `PolicyConfig` は `Default` を持たず、全項目を明示しなければ作れない。この予行演習で使う数値は、すべて `TEST_ONLY_HYPOTHETICAL_POLICY`（`binding = false`、「FINAL policy の候補値として引用しない」と明記）。凍結 policy の表現（`Frozen`）はあるが、この予行演習では作らない。
- **併記**: `n_auto` / AUTO coverage / abstention rate / 誤 AUTO の件数 / 下限 / 信頼水準と方式を、必ずレポートに載せる。評価単位ごとの値（キー・identity）は出さない（件数と率だけ）。

## 監査の指摘による補正（2 点）
1. **機械 = AUTO・GT = defer / unresolved** → `UnevaluableAuto` に `BlockEvaluation` を追加。**binding な FINAL policy で許すのはこれだけ**（`Frozen` の policy が `Exclude` / `CountAsWrong` を使うと、`invalid_policy` の技術的な失敗）。該当する単位が 1 件でもあれば、他の条件を満たしていても、評価全体が **INCONCLUSIVE**（理由 `auto_with_unresolved_gt`）。`Exclude`（precision を上方に選べる）と `CountAsWrong`（正誤不明を誤答と定義する）は、synthetic engine の試験にだけ残す。表示用の暫定値は `precision_binding = false` と明記し、件数 `n_auto_gt_unresolved` を併記する。機械が AUTO でない単位（REVIEW / UNRESOLVED）の GT が未解決でも影響しない。
2. **coverage と abstention の二重閾値をやめる。** 判定の空間では `AUTO coverage + abstention rate = 1` なので、`maximum_abstention_rate` を policy の項目から**削除**した（ソースにその語が無いことを検査）。**coverage が判定条件、abstention は必須併記。** `coverage_plus_abstention = 1` をレポートに載せ、テストで確認する。

## 判定の優先順位（固定）
技術的な失敗 → `TechnicalFailure` / AUTO の件数が最低に届かない、または AUTO の GT が確定できない → `Inconclusive`（閾値は緩めない。ほかの未達も理由に記録）/ それ以外の未達（coverage・誤 AUTO の上限・precision の下限）→ `NoPromotion` / すべて満たす → `Promote`。

## テスト（新規 20 件）
| 実行 | 結果 |
|---|---|
| `cargo test --lib pr4_pf0b` | 20 passed / 0 failed |
| `cargo test --lib`（全件の回帰） | 結果は `PR4_PF0B_FREEZE.txt` に記録 |
| `cargo check --lib` | 通過 |

確かめること: exact な下限（境界値・定義どおりの確率・単調性）/ precision の条件の PASS・FAIL / AUTO 件数だけ不足 → INCONCLUSIVE / coverage だけ不足 → NO PROMOTION / AUTO 0 件 / 1 件の誤 AUTO（上限の有無）/ none への誤 AUTO / REVIEW・UNRESOLVED は分母の外で abstention に入る / abstention は必須併記で二重の閾値ではない / 無効な AUTO・機械出力の欠落・実行の失敗は技術的な失敗で指標を出さない / membership の追加・削除・改名・版名の違い・重複・空を検出 / 順序に依存しない / AUTO の GT 未解決の扱い（3 通り）と binding policy が BlockEvaluation だけであること / 同じデータで policy を変えると判定だけが変わる / 不正な policy / 既定の policy が無い / test-only の表示 / レポートに評価単位の値が無い / DB・通信・ファイルに触れない / 混合した 100 件を 4 種類の架空 policy で通す。

## 限界
- 確かめたのは判定のロジック。凍結した機械出力の成果物を、このパイプラインの入力に変換する層は、まだ無い（policy freeze の後の「凍結版で機械出力を 1 回生成」の gate で作る）。
- 数値の policy（信頼水準・必要な下限・最低 AUTO 件数・最低 coverage など）は、PF0C で、結果を見る前に決める。この予行演習の数値は、その候補ではない。
