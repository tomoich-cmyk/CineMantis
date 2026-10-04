# Follow-up Issues

コミット前のレビューや実装中に見つかった、別途対応する項目。上から優先度順ではない。

## PR4 開始前の状態

- **C5c.5 は完了**（resolved 44 / 50、deferred 6 / 50。詳細は `docs/C5C5_CLOSEOUT.md`）。
- **holdout は未開封。** 昇格判断（rules-tags-shadow や Jev の本番昇格）は、まだ行っていない。
- 昇格判断に使う指標は **FINAL end-to-end AUTO precision**（holdout 全体での「正しい AUTO」÷「すべての AUTO」）。
  C5c.5 の recall は resolved GT だけを分母にした条件付きの値で、昇格には使わない。
- **policy freeze に当たる境界は、まだ定義も文書化もされていない。** 定義されるまで、live の holdout を開封してはならない。
- PR4 の実装（下の項目 3）は **未着手**。

## 監査指摘（PR2.5 レビュー時に記録）

### 1. 既存 awards 系の外部キー違反6件の調査・修正
`PRAGMA foreign_key_check` が実 DB で6件の違反を返す。すべて
`work_award_match_candidates` → `award_import_items`（fk id 1、rowid 32 / 39 / 45 / 49 / 62 / 68）。
021 以前から存在し、PR2 / PR2.5 とは無関係。022 の適用前後で件数は変わらない（6 → 6）ことを確認済み。
awards の取り込み処理が親行を消しているか、取り込み途中で中断した名残と思われる。原因を調べて掃除する。

### 2. PR3 開始前に rules-1 を完全凍結する — **一部完了**
状態を 2 つに分ける。
- **rules-1 の判定ロジック・検索計画の凍結: 完了**（`91b533d`）。
  `services/rules_v1.rs` に `parse_title_v1` / `search_plan_v1`（検索語の生成）/ `score_movie_v1` / `score_tv_v1` /
  `merge_and_rank_v1` / `best_candidate_v1` を版付きで固定した。rules-1 の verdict はそのコピーから出す。
- **TMDB 呼び出しを含む candidate generation の実行経路の凍結: 確認できていない（未確立）。**
  `rules_v1.rs` の冒頭コメントは「TMDB を呼ぶのは本番経路のまま」と書いており、実リクエスト・取得候補の組み立て・
  取得経路そのものを独立して凍結した証拠は、コードからは見つからない。
  元の項目にあった「candidate generation も含めて」という記述は、この点で範囲が広すぎた。
  実行経路まで凍結する必要があるかは、PR4 の前に判断する。

### 3. PR4 で reason code 別の集計を出す — **未着手（保留中）**
`metadata_match_runs.decision_reasons_json` と `metadata_match_verdicts` から、
- reason code ごとの AUTO 停止件数
- 母集団に対する REVIEW 率
- rules-safe と rules-tags-shadow の一致・不一致
を集計する画面（またはレポート）を作る。rules-tags-shadow の本番昇格判断はこの数字で行う。
ただし昇格の最終指標は FINAL end-to-end AUTO precision（上の「PR4 開始前の状態」）。この集計だけでは昇格を決めない。

### 4. 候補ごとの matcher 別 score / rank の分離 — **023 で実装済み（記述を更新）**
PR2.5 以降、`metadata_match_candidates.rules_score` / `rules_rank` は
**その run が保存した combined deterministic candidate set 上の最良スコアと順位**であり、
rules-1 固有でも rules-safe 固有でもない（同じ作品が旧経路と埋め込み検索の両方で
見つかった場合は高い方の点数を採るため）。各判定器の正式な結論・top candidate・score は
`metadata_match_verdicts` を正とする。

`023_jev_shadow.sql` が、候補ごとの matcher 別の値を持つ `metadata_match_candidate_scores` を追加した。
- `matcher` は `legacy`（旧経路 = rules-1 の採点）/ `rules-tags-shadow` / `jev-call` の 3 通り。
  guard 込みの rules-safe は、候補スコアの出どころには使わない。
- 採点式の版を `matcher_version` に必ず持ち、`in_candidate_set` で「入力に入っていない候補」と
  「入っていたが選ばれなかった候補」を区別する。
- 後埋めは `023_jev_shadow.sql` ではなく `db.rs` の `backfill_legacy_candidate_scores` が行う（起動時）。
  `query_source='legacy'` の候補だけに旧経路の score を移し、**rank は移さない**（NULL）。
  rules-tags-shadow と jev-call の過去分は後埋めしない。
- **rules-safe と rules-tags-shadow について、matcher 別の候補 rank を書く production の経路は無い。**
  `candidates.rules_rank` は統合集合上の順位のまま（PR4-0 の棚卸し: `docs/pr4_schema_inventory.json`）。

注記: 候補ダイアログは統合後のスコアを表示し続けてよい。ただしこの数値を
production AUTO の確信度として扱わないこと。

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
