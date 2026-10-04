# PR4-2: 診断用 GT の意味契約 — 報告

状態: **契約を凍結。コードの実装なし。** holdout は封印のまま。DB は読んでいない。通信なし。commit していない。
HEAD `407623180210d29672a74d56a8290b4c66b0f13f`（変更前後で同じ）。変更は docs の新規ファイルだけ。アプリのコード・migration・PR4-0 / PR4-1 の凍結ファイルは変えていない。

## 決着した 3 点

| 未決事項 | 決定 |
|---|---|
| `label='none'` | **評価可能な negative GT。** 信頼できる `review_none` に対して AUTO が候補を選べば誤りで、分母に入る。TMDB-positive 専用の指標では分母外 |
| 統合集合の top-3 rank | **precision には使わない**（`top3_for_auto_precision = NOT_APPLICABLE`）。正しさは `(media_type, tmdb_id)` の完全一致だけ。top-3 は将来の別指標で、同じ候補集合の rank だと証明できる場合だけ |
| production の `manual_*` | **GT として採用しない**（`untrusted_manual_gt_source`）。明示的な allowlist（§5）を満たすラベルだけが信頼できる |

そのほか固定したもの: `defer` は課題の state（ラベルではない）で未解決として除外 / 衝突規則（最新ラベル優先はしない、`gt_conflict`、`gt_superseded`、`gt_multiple_units_per_work`）/ 分母・分子・正しさの定義 / 分母 0 は `null` / FINAL とは別。

## 実在するラベルの棚卸し

`labels.label` は `tmdb` / `none`。`method` は 7 値（`manual_apply` / `manual_direct_id` / `lock` / `review_confirm` / `review_pick_other` / `review_none` / `bulk_accept`）。`lock` と `bulk_accept` は CHECK で weak。GT として許すのは `review_confirm` / `review_pick_other` / `review_none` の strong だけ（allowlist の条件つき）。`bulk_accept` は非テストの書き込み元がなく、`LabelMethod` にも無い。`defer` はラベルでなく課題の state。一覧は `PR4_DIAGNOSTIC_GT_CONTRACT.md` §6 と `pr4_gt_semantics.json`。

## 検証（本番 DB・holdout なし）

1. **参照評価器で 25 ケース — 25 通過。** 指示の必須 12 ケース（positive 一致 / 不一致、none + AUTO、defer + AUTO、GT なし + AUTO、manual + AUTO、衝突、REVIEW 機械状態、UNRESOLVED 機械状態、分母 0、top-3 一致でも選択が違う、holdout fixture）と、境界 13 ケース（同一の重複 GT、信頼 + 非信頼、provisional sample、superseded、weak lock、bulk_accept、未知の method、`tmdb_verified` の無い pick_other、検証済み pick_other、1 作品に複数単位、media_type も identity の一部、AUTO なのに identity なし、matcher ごとの precision）。評価器はリポジトリには入れていない（合成した評価単位だけを扱う、契約確認用）。
2. **契約が依拠する事実 19 件を、コードとスキーマに対して静的に検証 — 19 通過。** 021 の CHECK（ラベル値・method・strength の対応・none の identity・作品ごとに有効 1 件）、`gt_review.rs` の `resolve`（label へ `review_task_id` と `task.run_id`、task へ `resolution_label_id`、ready のみ確定、既存 strong ラベルがあれば作らない、pick_other の `tmdb_verified`）、`record_label` が active ラベルを strong も含めて superseded にすること、manual / lock の発生元、`bulk_accept` の書き込み元なし、`docs/C5C5_CLOSEOUT.md` の sample 記述。
3. 検証結果の全文は `pr4_gt_semantics.json` の `validation`（ケース別・事実別）。

## 要注意（PR4-3 の前に）

- **D / E / F の解決は DB のラベルではない。** 71, 93, 96, 104 などは、DB 上は未解決として数えられる。取り込みの protocol は未定義（PR4-3 では扱わない。別の amendment が要る）。診断 precision の分母は、DB に残っている allowlist の GT だけになる。
- allowlist は `c5c5c-redo-01〜05`（`docs/C5C5_CLOSEOUT.md` に由来）。出所未確定の `c5c5-dev-expand-01` は構造的に review_* でも入らない。
- 後続の manual ラベルで上書きされた GT は保守的に除外（`gt_superseded`）。

## 守った条件

NONE の意味を固定 / top-3 の問題を決着 / manual_* の方針を決着 / 実在ラベルを棚卸し / 信頼できる GT の allowlist を明示 / 衝突規則を明示 / 分母と正しさを明示 / holdout は封印 / FINAL は NOT_RUN / 本番コード不要（実装していない）。

## 次

PR4-3: 診断 precision の実装（PR4-1 の report の `diagnostic_gt` に実装）。holdout と FINAL には触れない。
