# PR4-PF0A: holdout の件数と所属の commitment（count-only）

状態: **手順と道具の設計・実装。** holdout を開封する作業ではない。性能の結果は見ない。FINAL は NOT_RUN。policy freeze は未定義。
前提: 監査（grill-me）の結論は「NOT READY FOR POLICY FREEZE」。開発用 GT の 50 件では、rules-safe / rules-tags-shadow の AUTO が 0 件（分母 0）。これは「機械の判定を見て難しい例だけを選んだ」偏りではなく（抽出条件に機械の判定は無い）、**開発用 GT の cohort と、最終的に評価したい AUTO の領域の support がほぼ重ならない**という代表性のずれ。開発用 GT を後付けで足して分母を作らない。

## 目的
いまの holdout の規模を、**性能の結果を一切見ずに**確認し、現在 eligible な評価単位の集合を commitment（ハッシュ）で固定する。この規模なら診断用途に留めるか、事前のルールで追加の holdout を蓄積するかを、結果を見ずに決めるための材料にする。

## 出力（これだけ）
`holdout_source_count` / `excluded_ever_strong_count` / `current_eligible_count` / `membership_commitment_sha256` / `eligibility_rule_version` / `snapshot_sha256`（+ 実行の記録: protocol、時刻、source HEAD、制約の注記）。
- `holdout_source_count`: live cohort の作品数（既存の `COHORT_CTE`）。
- `excluded_ever_strong_count`: そのうち strong ラベルが**一度でも**付いた作品の数（superseded を含む。既存の抽出の述語と同じ。ラベルは `EXISTS` の存在判定にだけ使い、列の値は読まない）。
- `current_eligible_count`: 既存の `build_frame(conn, "live", "")` が返す件数（file あり・利用可・source が online・他の sample で未抽出・strong ラベルなし）。
- `membership_commitment_sha256`: 現在 eligible なキー `work:<work_id>` を辞書順に並べ、`pr4-pf0a-membership-1\n` + `eligibility_rule_version\n` + 各キー + `\n` を SHA-256 にしたもの。**キーの一覧は出力・保存・表示しない**（メモリ内でハッシュにだけ使う）。
- `eligibility_rule_version`: `pr4-pf0a-eligibility-1/gt_sampling_sha256=<述語を含むソース全体の SHA-256>`。
- キーが `work_id` なのは、holdout がまだ抽出されておらず、評価単位のキー `{sample_id, task_id, work_id}` が存在しないため（抽出時の frame の主キーが work_id）。

## 禁止
題名・work ID・task ID の一覧 / verdict / candidate / machine decision / ラベルの値・method / GT の内容 / 評価単位ごとの結果 / precision の計算。述語の再実装をしない（`gt_sampling.rs` の述語と `COHORT_CTE` を、そのままソースから使う）。

## 実行の方法
PR4-3V1 と同じ。production の DB を SQLite として開かず、停止中の DB / WAL / SHM を byte copy → コピー側で recovery → immutable snapshot の上で実行する（読み取り専用 + `immutable=1` + `query_only`）。production の DB のパスを受け取る入口は無い。production が実行の前後で 1 byte も変わっていないことを確認する（変わっていれば STOP）。既に取得済みの snapshot を使う場合は、production のフィンガープリントが、その snapshot の取得時と**一致する**ことを、通常のファイル読み取りだけで確認する。

## 固定すること（policy freeze に入れる）
**この commitment を作った時点から、最終評価の終了まで、membership を変えない。** production がその後変化しても（通常利用で作品が追加される・ラベルが付く・ファイルが移動するなど）、最終評価はこの commitment と snapshot の境界を基準にする。

## 結果の使い方
件数だけで、次を決める（性能は見ない）: この規模で診断用途に留めるか / 追加の holdout を事前のルールで蓄積するか。holdout の件数が少なければ、片側 95% の下限は、全 AUTO が正解でも、小さな値にしか届かない（n 件で全正解のとき `0.05^(1/n)`。n = 25 で約 88.7%）。保証水準と必要な AUTO 件数は、結果を見る前に別 gate で決める。
