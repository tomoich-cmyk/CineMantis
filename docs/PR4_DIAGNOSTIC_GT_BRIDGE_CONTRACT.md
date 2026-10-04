# PR4-2A: 診断用 GT の provenance bridge 契約

状態: **契約を凍結。bridge を 1 つ作成して凍結（リポジトリの外）。** production DB への書き込みなし。holdout は読んでいない。診断 precision は計算していない。
機械可読スキーマ: `docs/pr4_gt_bridge_schema.json`（`bridge_version = pr4-diagnostic-gt-bridge-1`）。
前提: PR4-2（`docs/PR4_DIAGNOSTIC_GT_CONTRACT.md`）。PR4-2 の凍結ファイルは書き換えない。この文書はその amendment 層で、PR4-3 が使う GT の供給元を定める。

このリポジトリの文書には、GT の中身（task ID、TMDB の作品 ID、題名、候補、画像）を書かない。書くのは、schema・出典 artifact の名前と SHA・件数・規則だけ。

## 1. 目的

C5c.5 の正式な解決は 44 件（resolved 44 / deferred 6）。しかし D / F で解決した分は、DB のラベルになっていない（監査ディレクトリの判断で、DB には書いていない）。PR4-2 の allowlist だけから診断 precision を作ると、GT の集合が正式 closeout と一致しない。
この bridge は、**DB に無い正式な解決済み GT だけ**を、凍結済みの closeout / decision artifact から取り出した read-only の sidecar で、DB には何も書き戻さない。

## 2. GT の供給元は 2 つ。優先順位はない

| 供給元 | 中身 |
|---|---|
| **TRUSTED_DB_GT** | PR4-2 の allowlist を満たす DB の strong ラベル（`review_confirm` / `review_pick_other` / `review_none`） |
| **FROZEN_OFF_DB_GT** | この bridge。凍結された C5c.5 の artifact にある、DB に無い正式な解決済み GT だけ |

- 同じ評価単位を両方が指すとき: **同じ canonical GT → 1 件に畳み、provenance を両方残す / 食い違い → `gt_source_conflict`（fail closed）。**
- 「最新が勝つ」はしない。off-DB は DB を上書きしない。DB は off-DB を上書きしない。供給元の間に supersession の優先関係を作らない。DB 内の supersession の意味は PR4-2 のまま。

## 3. bridge に入れるもの・入れないもの

- 入れる: 解決済み（POSITIVE）の GT だけ。評価単位のキー、`(media_type, tmdb_id)`、出典（段階・artifact 名・完全な SHA-256・protocol 版）、`provenance_class = FROZEN_OFF_DB_GT`。
- **入れない: `defer` / 未解決 / 候補 0 件 / 検索の miss / 保留中の task**（GT ではない）。題名・概要・画像・候補の順位・機械の score・Jev・無関係なレビュー証拠も入れない（スキーマが `additionalProperties=false` で拒否する）。
- DB にある GT を複製しない（bridge は off-DB の追加分だけ）。
- 将来、凍結された正式な artifact が「該当なし」を明示的に含む場合に限り `NONE` を表現できる。defer・候補 0 件から NONE を推測しない。

canonical な GT の表現（両供給元を同じ形にそろえる）:

| semantic | media_type | tmdb_id |
|---|---|---|
| `POSITIVE` | `movie` \| `tv` | 整数（> 0） |
| `NONE` | null | null |

off-DB の `confirm` / `confirm_existing_candidate` は `POSITIVE`。DB の `review_confirm` / `review_pick_other` は `POSITIVE`、`review_none` は `NONE`。`defer` は GT のレコードを持たない。

## 4. 評価単位のキー

**キー = `{sample_id, task_id, work_id}`**（3 成分すべて必須。1 つでも欠けたら STOP）。`reviewed_run_id` は属性で、キーの成分ではない（DB の `task.run_id` と一致しなければ `gt_source_conflict`）。

選んだ根拠（便利だからではなく、両側に存在し一致することを確かめた）:
- DB 側: `metadata_review_tasks.id`、`.work_id`、`sampling_json.sample_id`（抽出時に固定される記録）。
- off-DB 側: D の Phase 0 manifest の項目（`task_id`、`work_id`、`run_id`、`sample_id`）。manifest の SHA は D Phase 2 decisions の provenance に固定されている。
- 証明: 凍結した C の final snapshot（SHA 固定・immutable・read-only）で、manifest に載る formal な開発用 task の**メタデータ行だけ**（id / work_id / run_id / sample_id）を読み、10 件すべてで 4 つの値が一致、キーの組は 10 件とも一意だった。ラベル・verdict・候補・holdout は読んでいない。production DB は開いていない。証明の出力は外部の `key_proof.json`。
- キーは候補の rank にも、変わりうる時刻にも依存しない。

## 5. 出典 artifact と SHA の固定

- 根の anchor（完全な SHA-256）:
  - `C5c5_FINAL_CLOSEOUT.json` = `31c30915d3d21db90b06fb40334787ab041e818b5a935a14e5f4c2dde9bd9c8b`
  - `F_CLOSEOUT.json` = `172c797196c7896cd1689fc187a005aaef3b1eb9f88edc7027b1f61150c55ead`
- そのほかの出典（D の decisions、F の 5 つの正式 decision、E の decision と closeout、C の閉鎖記録、D の Phase 0 manifest と candidate manifest）の SHA は、FINAL closeout / F closeout / D decisions の provenance に記録された**完全な値**を使う。記憶・短縮形・Downloads のコピーは権威にしない。
- builder は、SHA が 64 桁の小文字 16 進でない・実ファイルと違う、のどちらでも出力を作らず STOP する。

## 6. 導出の規則（task ID を直書きしない）

1. FINAL closeout から会計（母集団 / resolved / deferred、段階別の件数と ID）を読む。
2. D の decisions・F の各 decision から、解決（`confirm`）を**独立に**導く。D は candidate manifest で identity を引く（task が一致すること）、F は選択された identity を使う。
3. closeout の会計と突き合わせる: D の解決の集合、F1-A の解決の集合、E は 0、F の他の段階は 0、重なりなし、`v1 の deferred = D + F + 最終 deferred`、off-DB の件数 = `resolved − v1 の resolved`。1 つでも合わなければ STOP。
4. 件数（現在 4）は導出の**結果に対する整合性の検査**であって、record を選ぶ手段ではない。

## 7. 将来の merge アルゴリズム（PR4-3 が実装する）

1. PR4-2 の allowlist で TRUSTED_DB_GT を得る。
2. bridge を読む。`status` が `READY` でなければ PR4-3 は実行しない。
3. 両方を canonical GT にする。
4. キー `{sample_id, task_id, work_id}` で結合する。
5. 両方にあり canonical GT が同じ → 1 件。provenance を両方残す。
6. 両方にあり canonical GT が食い違う、または `reviewed_run_id` が食い違う → `gt_source_conflict`。その単位は GT にせず、`diagnostic_gt.status = BLOCKED`（precision を出さない）。
7. bridge にだけある → 含める。DB にだけある → 含める。
8. defer / 未解決 → GT レコードなし。
9. 結合後の GT の単位数は、凍結した closeout の resolved（44）と一致しなければならない。一致しない場合は `gt_universe_mismatch` として `BLOCKED`（DB 側の信頼できる GT が期待の件数に満たない、などを黙って受け入れない）。
10. DB には何も書き戻さない。

bridge の同一評価単位の重複は、内容が同一でも `duplicate_unit_identical` として拒否（黙って畳まない）、食い違えば `gt_source_conflict`。

## 8. closeout の会計との整合

formal な母集団は 50、resolved 44、deferred 6。内訳: v1（DB の GT）40、D の新規 3、E の新規 0、F の新規 1。bridge は D + F の 4 件を独立に導出し、会計と一致した。historical な C / D / E / F の指標は書き換えていない。

## 9. holdout の隔離

bridge は C5c.5 の正式な開発 sample 群（`c5c5c-redo-01〜05`）の GT だけ。holdout の task / ラベル / verdict / 候補を読まず、含まない。実際の holdout の所属で試験していない。formal な出典の範囲から構造的に検証した（sample が群の外なら reject）。

## 10. bridge の状態（PR4-3 向け）

```
diagnostic_gt_bridge:
  status: READY
  bridge_version: pr4-diagnostic-gt-bridge-1
  off_db_records: 4
  expected_final_resolved: 44
  expected_final_deferred: 6
  production_db_modified: false
  holdout_read: false
```

実体の `diagnostic_gt_bridge.json` はリポジトリの外（`CineMantis-audit\pr4\gt_bridge\`）。リポジトリに入れる判断は別途。FINAL の昇格指標は NOT_RUN（`HOLDOUT_SEALED_POLICY_NOT_FROZEN`）のまま。
