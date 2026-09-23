# Follow-up Issues

コミット前のレビューや実装中に見つかった、別途対応する項目。上から優先度順ではない。

## 監査指摘（PR2.5 レビュー時に記録）

### 1. 既存 awards 系の外部キー違反6件の調査・修正
`PRAGMA foreign_key_check` が実 DB で6件の違反を返す。すべて
`work_award_match_candidates` → `award_import_items`（fk id 1、rowid 32 / 39 / 45 / 49 / 62 / 68）。
021 以前から存在し、PR2 / PR2.5 とは無関係。022 の適用前後で件数は変わらない（6 → 6）ことを確認済み。
awards の取り込み処理が親行を消しているか、取り込み途中で中断した名残と思われる。原因を調べて掃除する。

### 2. PR3 開始前に rules-1 を完全凍結する
`score_movie` / `score_tv` / `best_candidate` に加えて、**検索語の生成と candidate generation も含めて**
`services/rules_v1.rs` に凍結コピーを作る。PR3 で parser を変えると、今の実装を共有したままでは
比較基準が動いてしまう。凍結後は rules-1 の verdict をそのコピーから出す。

### 3. PR4 で reason code 別の集計を出す
`metadata_match_runs.decision_reasons_json` と `metadata_match_verdicts` から、
- reason code ごとの AUTO 停止件数
- 母集団に対する REVIEW 率
- rules-safe と rules-tags-shadow の一致・不一致
を集計する画面（またはレポート）を作る。rules-tags-shadow の本番昇格判断はこの数字で行う。

## 実装中に見つかったもの（PR1 / PR2 / PR2.5）

- migration 015 が `db.rs` に繋がっていない（存在するが適用されない）
- TMDB のロゴ画像を未配置（配布物のダウンロード許可が必要）
- 一括操作のスキップ件数を UI に表示していない
- 既存の `match_confidence = 100` が 1,046 件（旧実装の名残。意味を持たない値）
- `works.title_guess` は 021 以前の作品では NULL（照合時にファイル名から復元している）
- 一括照合は1回 10 件で止まる（`LIMIT 10`）
- 古い run の間引き（作品ごとに最新3件だけ残す prune）が未実装
- 照合履歴・レビュー課題・タグの矛盾を見る UI が無い（PR4 で対応）
- NAS 振り分けの接続確認は入れたが、移動中に NAS が落ちた場合の再開手順は未整備
