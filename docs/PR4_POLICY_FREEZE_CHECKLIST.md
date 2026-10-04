# PR4: policy freeze チェックリスト（holdout を開く前に凍結するもの）

状態: **policy freeze は未定義。** このリストの NOT FROZEN / UNKNOWN が残っている間、live の holdout を開封してはならず、最終評価を始めてはならない。
分類: **FROZEN** = 版つきで固定された成果物がある / **NOT FROZEN** = 未固定 / **NOT APPLICABLE** = 今回の評価対象では不要 / **UNKNOWN** = コードや文書から確かめられない。
`search_plan_v1` があることを根拠に、候補生成の実行経路を「凍結済み」とはしない。

| # | 項目 | 分類 | 根拠・メモ |
|---|---|---|---|
| 1 | 評価対象の判定器（rules-safe-2 / rules-tags-shadow-2 / Jev のどれを昇格対象にするか） | **UNKNOWN** | 文書に指定が無い。FOLLOWUPS は rules-tags-shadow の昇格を想定していたが、昇格の指標は FINAL precision のみ |
| 2 | rules-1 の実装 | **FROZEN**（判定ロジックと検索計画） | `services/rules_v1.rs`（`91b533d`）。ただし `matcher_version='rules-1'` は凍結の前後で同じ文字列 |
| 3 | rules-safe / rules-tags-shadow の実装 | **NOT FROZEN** | 共有の `metadata_matcher.rs`。変更時に version 文字列を上げる運用はあるが、評価用に commit SHA を固定する仕組みが無い（audit sample の `sampling_json.source_git_sha` は抽出時の記録だけ） |
| 4 | rules / profile の版 | **NOT FROZEN** | `rules-safe-2` / `rules-tags-shadow-2` / `rules-1` / `jev-policy-1` は存在するが、「この版の組で評価する」という宣言が無い |
| 5 | 候補生成の実行経路（TMDB 検索・取得・統合・候補集合の作り方） | **NOT FROZEN** | `rules_v1.rs` 冒頭が「TMDB を呼ぶのは本番経路のまま」と明記。候補集合は TMDB の応答と検索順に依存し、時間で変わりうる |
| 6 | AUTO / REVIEW の決定ポリシー | **NOT FROZEN** | `rules_core` は実装として存在（理由が空なら AUTO）。評価対象のポリシーとして指定されていない |
| 7 | 理由コードの語彙とマッピング | **NOT FROZEN** | `metadata_matcher::reason` の定数（現行 14 個）。独立した版が無い。理由に優先順位の定義も無い |
| 8 | 閾値 | **NOT FROZEN** | `THRESHOLD_AUTO` / `THRESHOLD_CANDIDATE` / `YEAR_TOLERANCE` は run ごとに `policy_snapshot_json` へ記録される（証跡）が、評価用の確定値の宣言は無い |
| 9 | 同点処理 | **NOT FROZEN** | 最大スコアは「先に出た候補」を採る（厳密な `>`）。同点は `TIED_TOP` で REVIEW になる。候補の順序が検索結果に依存するので、再現性は UNKNOWN |
| 10 | abstention の挙動 | **NOT FROZEN** | 指標の定義（REVIEW + UNRESOLVED）は `audit_run.rs` のコメントと PR4-0 契約にある。FINAL の分母で abstain をどう扱うかは未定義 |
| 11 | GT の比較ルール | **NOT FROZEN** | `label='tmdb'` は `(tmdb_id, media_type)` の一致が自然だが、`label='none'` の扱い・統合 rank を top-3 に使う可否は未定義 |
| 12 | FINAL の分子の定義 | **NOT FROZEN** | コードのコメントは「正しい AUTO」。正しさの定義が #11 に依存する |
| 13 | FINAL の分母の定義 | **NOT FROZEN（BLOCKED BY POLICY FREEZE）** | コメントは「すべての AUTO（holdout 全体）」。none ラベル・GT 未解決・abstain の扱いが未定義。PR4-0 は分母を発明しない |
| 14 | 除外ルール | **NOT FROZEN** | 診断用の除外は PR4-0 契約にあるが、FINAL 用は未定義 |
| 15 | holdout の所属 | **UNKNOWN** | live は holdout 専用という取り決めはあるが、所属の凍結 sample が存在するかは未確認（DB を読んでいない）。通常 report で live を他と合算しないことで当面しのぐ |
| 16 | holdout の 1 回限りの開封手順 | **NOT FROZEN** | `gt_operator.rs` は live を development に回す入口を持たない（開封の入口は無い）。開封の手順・記録・再開封の禁止は未定義 |
| 17 | FINAL の report スキーマ | **NOT FROZEN** | 通常 report の形は PR4-0 で凍結（`pr4_metric_contract.json`）。FINAL 用は別 |
| 18 | 昇格の閾値・判断ルール | **NOT FROZEN**（存在しない） | リポジトリに昇格の閾値や規則は無い（コード・文書を検索して確認）。GT の必要件数や AUTO 件数の目安も、このリポジトリには書かれていない |
| 19 | Jev の contract / model の固定 | **NOT APPLICABLE**（今は） | session ごとの model pin と contract の canonical SHA の仕組みはある。Jev は AUTO を出さず、PR4 の通常指標の入力にしない。Jev を昇格対象にするなら適用対象になる |
| 20 | 開発 GT と holdout の分離ルール | **NOT FROZEN**（規約と検出のみ。DB では守られていない） | live は holdout 専用で development に使えない（`gt_sampling::validate`、サービス層）。`sampling_json`（`purpose` / `cohort` を含む）は「作成後に更新しない」規約で、**DB の制約・トリガーでは保護されていない**。書き換えは `freeze_sample` / `load_manifest` が読むときに検出して拒否する（`SampleConfigMismatch` / `CorruptManifest`）が、直接の SQL 更新自体は止まらない。holdout を守る前提として、policy freeze の前に DB 側の保護か、開封前の整合検査を決める |

## 開封の前提（これが全部満たされるまで holdout は開けない）

1. 評価対象の判定器と版の組（#1, #4）を宣言する。
2. 実装の同一性（#2, #3）を commit SHA で固定する。
3. 候補生成の実行経路（#5）を、凍結するか、評価の外にあると明示する。
4. 決定ポリシー・閾値・同点処理・理由コード（#6〜#9）を固定する。
5. GT の比較ルールと FINAL の分子・分母・除外・abstain の扱い（#10〜#14）を文書で決める。
6. holdout の所属と 1 回限りの開封手順（#15, #16）、FINAL の report スキーマ（#17）、昇格ルール（#18）を決める。
7. holdout と development の分離を DB 側でも守るか、開封前の整合検査で担保するかを決める（#20）。
