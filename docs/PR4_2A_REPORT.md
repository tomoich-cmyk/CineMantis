# PR4-2A: 診断用 GT の provenance bridge — 報告

状態: **契約を凍結し、bridge を 1 つ作成・凍結した（リポジトリの外）。** production DB への書き込みなし。holdout は読んでいない。通信なし。診断 precision は計算していない。commit していない。
HEAD `687adb163a8c1e03b7761b7c2a4d3be98160edac`（前後で同じ）。リポジトリの変更は docs の新規ファイルだけ（アプリのコード・migration・PR4-0 / PR4-1 / PR4-2 の凍結ファイルは変えていない）。
このリポジトリの文書に GT の中身（task ID・TMDB の作品 ID・題名・候補・画像）は書いていない。

## 結果

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

## 決めたこと・確かめたこと

- **供給元は 2 つで優先順位なし。** TRUSTED_DB_GT（PR4-2 の allowlist）と FROZEN_OFF_DB_GT（この bridge）。同じ canonical GT → 1 件に畳み provenance を両方残す / 食い違い → `gt_source_conflict`（fail closed。その単位は GT にせず `diagnostic_gt.status = BLOCKED`）/ 片方だけ → 含める。「最新が勝つ」なし。off-DB は DB を上書きしない、DB は off-DB を上書きしない。
- **評価単位のキー = `{sample_id, task_id, work_id}`。** 3 成分すべて必須。`reviewed_run_id` は属性。両側に存在し一致することを、凍結した C の final snapshot（SHA 固定・immutable・read-only）の**メタデータ行だけ**（10 件）で証明した。10/10 一致、キーは一意。ラベル・verdict・候補・holdout は読んでいない。
- **bridge は解決済み（POSITIVE）の GT だけ。** defer・未解決・候補 0 件・検索の miss は GT にしない。題名・概要・画像・rank・score・Jev は入れない（スキーマが拒否）。DB にある GT は複製しない。
- **task ID を直書きしない。** FINAL closeout の会計（母集団 50 / resolved 44 / deferred 6、段階別の件数と ID）を読み、D の decisions・F の各 decision から解決を**独立に**導いて突き合わせた。結果は D 3 + E 0 + F 1 = 4 件で、`resolved(44) − v1(40)` と一致。v1 で deferred だった 10 件 = D 3 + F 1 + 最終 deferred 6、重なりなし。E は新規解決 0、F は F1-A のみ。4 件という数は導出の結果への整合性の検査で、record の選択には使っていない。
- **SHA はすべて完全な 64 桁。** 根の anchor は FINAL closeout `31c30915…c9c8b` と F closeout `172c7971…c55ead`（指定どおり）。そのほか 11 件の出典の SHA は、これらの記録（FINAL closeout / F closeout / D decisions の provenance）にある完全な値を使い、実ファイルと突き合わせた（計 13 件）。builder は、SHA の形式不正・不一致で STOP する。
- **supersession:** DB 内の意味は PR4-2 のまま。cross-source の優先関係は作らない。
- **合流後の整合:** PR4-3 は、結合後の GT の単位数が closeout の resolved（44）と一致することを検査し、満たさなければ `gt_universe_mismatch` で `BLOCKED`（DB 側の信頼できる GT が足りないことを黙って受け入れない）。本 gate は DB を読まないので、DB 側の実際の件数は未確認（PR4-3 が実行時に確認する）。

## 検証

- 外部のテスト `test_build_diagnostic_gt_bridge.py`: **40 / 40 通過**（指示の 15 ケース + 追加 25 件）。15 ケース: trusted DB の positive / NONE、off-DB の positive、同一の DB・off-DB、DB と off-DB の衝突、defer、identity なし、SHA 不正、closeout の分割が違う、同一の重複、衝突する重複、キーの成分欠落、sample が範囲外、件数が会計と合わない、title / rank / score / image を含む bridge。追加: 実際の bridge の検査（スキーマ適合、決定性、task ID 直書きなし、DB・通信を使わない、E / F の段階の検査）。実 socket の接続 0 回。
- 実際の bridge は、リポジトリのスキーマ（`additionalProperties=false`）に適合。スキーマと検証コードは同じ定数から生成している。
- 最初の実行で、5b（優先順位がないこと）の検査が失敗した。役割を入れ替えると出典のラベルも入れ替わるのに、全体の等価を比べていた、テスト側の誤り。判定（GT と衝突）だけを比べるよう直した。また、常に真になる式が 2 つ混じっていたので取り除いた。

## 成果物

リポジトリ（docs）: `PR4_DIAGNOSTIC_GT_BRIDGE_CONTRACT.md`、`pr4_gt_bridge_schema.json`、`PR4_2A_REPORT.md`、`PR4_2A_FREEZE.txt`。
外部（`CineMantis-audit\pr4\gt_bridge\`）: `diagnostic_gt_bridge.json`、`build_diagnostic_gt_bridge.py`、`test_build_diagnostic_gt_bridge.py`、`prove_unit_key.py`、`key_proof.json`、`gen_bridge_schema.py`、`BRIDGE_REPORT.txt`、`BRIDGE_FREEZE.txt`。
実体の bridge をリポジトリに入れる判断は別途（今回は入れない）。

## 守った条件

キーを証明 / off-DB の D / F の GT を実際に作成 / 出典 SHA は完全 / 解決済み GT だけ / 導出した件数が closeout と一致 / 44・6 の会計を維持 / 衝突規則を固定・優先順位なし / production DB は変更なし / holdout は未接触 / リポジトリの契約文書を凍結 / 外部 bridge を凍結 / 診断 precision は未計算。FINAL の昇格指標は NOT_RUN（`HOLDOUT_SEALED_POLICY_NOT_FROZEN`）、policy freeze は未定義。

## 次

PR4-3: 診断 precision の実装。GT の供給元は `TRUSTED_DB_GT + FROZEN_OFF_DB_GT` の結合 view。bridge の `status = READY` を実行の前提にする。
