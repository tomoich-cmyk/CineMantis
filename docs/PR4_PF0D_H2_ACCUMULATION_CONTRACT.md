# PR4-PF0D: H2 蓄積契約 / feasibility census（凍結）

状態: 契約を凍結。H2 は未収集。holdout は開いていない。FINAL は NOT_RUN。機械可読版: `docs/pr4_h2_enrollment_rules.json`。

## 0. PF0C との優先関係（amendment の範囲）
```
PF0C statistical/promotion thresholds remain binding and unchanged.

PF0C sample-size sizing based on H1+H2 is superseded by PF0D
for holdout accumulation design only.

Primary FINAL holdout = H2 only.

Therefore:
binding minimum H2 eligible = 598
operational target H2 eligible = 946

This does NOT amend:
- 95% one-sided exact Clopper-Pearson
- 99% required lower bound
- minimum AUTO count = 299
- minimum AUTO coverage = 50%
```
統計 policy の変更ではなく、holdout 母集団設計の amendment である。

## 1. 変更点（PF0C の sizing の修正）
- H1（25 件・commitment `22cc00a8…983e031`）は **legacy sealed diagnostic tranche**。封印・commitment を維持し、**primary の promotion 指標には使わない**。FINAL 終了後に secondary diagnostic として見る余地は残す。
- 根拠（membership の事実だけ）: H1 は source 28 件から ever-strong 述語で 3 件が除外されて 25 件になった。この述語は事後の情報に依存し、手動訂正が機械の失敗と相関しうる。3 件の中身（誤マッチだったか等）は解釈しない。
- **Primary FINAL holdout = H2 のみ。** binding minimum = **598** eligible、operational target = **946** eligible。PF0C の統計 policy（最低 AUTO 299・coverage 50%・片側 95% CP 下限 ≥ 99%）は不変。`pr4_final_policy.json` の `sample_size`（H2=573）は sizing について本契約で置き換わる（JSON は SHA を固定しているため編集しない）。

## 2. H2 の規則
1. 作成（初観測）時に登録し、以後は外さない。strong ラベル・手動訂正・matched/locked・matcher 出力の変化で除外しない。ラベルの有無を selection に一切使わない（月次ツール実行時点で付いていたから除外、も不可）。
2. 登録時点の入力 identity と matcher 入力フィールドの canonical hash を保持する。
3. T0 = PF0D の契約と enrollment ツールを freeze して commit・push した後の最初の enrollment run の UTC 時刻。遡及登録なし。
4. 登録時刻は外部 ledger の `first_seen_at`。`works.created_at` はコード上は更新箇所が無いが、schema/trigger の保護が無く（バックアップ復元等で変わりうる）、不変は証明できないため正式な時刻にしない。
5. 母集団: T0 以降に初めて観測された live cohort の作品。H1・C5c.5・過去 GT との重複は登録時に一度だけ除く。包含は work id 順の決定的規則で、裁量なし。
6. ledger は audit 側の append-only・hash chain の tranche commitment。production DB には membership を書き戻さない。登録後は通常の開発・GT・tuning から隔離し、backflow 無し。
7. 946 件に達するまで機械出力・性能を見ない。
8. GT は、機械出力を凍結した後、H2 全件を機械状態を隠して同一 protocol で blind review する。AUTO かつ GT 未解決が 1 件でもあれば INCONCLUSIVE（PF0C）。**feasibility requirement: 946 件を処理できるレビュー体制が前提。**
9. 供給が遅い場合は待つ。月次の count-only census（新規 entry 数・累計・目標までの残数）のみ可。99%・50%・946・母集団の緩和、途中での既存作品の追加は禁止。

## 3. Feasibility census（2026-10-06 実行・1 回）
スナップショット `6b805a13…75967`（production 未接触）。live cohort 28 / ever-strong 3 / eligible 25（H1 と commitment 一致 = H1 の外に現 eligible は無い）。作成月はすべて 2026-09、直近 12 か月平均 2.3 件/月だが窓の大半が空で過小評価。流入は 1 か月分では決まらない。946 件までの期間は未確定で、月次 census で追う。出力: `CineMantis-audit\pr4\pf0d\20261006T230131Z\h2_feasibility_census.json`。
注: census ツールの `eligible_now` は H1 時代の述語（ever-strong 除外）の件数で、H2 の selection には使わない。H2 の enrollment ツールは未実装（次の gate）。

## 4. 次の gate（未着手）
enrollment ツール（ledger・input hash・tranche commitment）の実装と freeze → T0 → 蓄積 → machine output 凍結 → blind GT → FINAL。

## Amendment（PF0E pre-T0 監査）
§2 の 1「作成時に登録」と 4・5 の表現は、PF0E で次の意味に置き換わる: **enrollment event = T0 の後の最初の scheduled observation で、非 baseline の work が target live population にいると観測された時点**（`first_seen_at`）。母集団の境界は baseline の work id 集合で決め、cadence は DAILY。詳細は `PR4_PF0E_ENROLLMENT_LEDGER_CONTRACT.md` の Amendment と `pr4_h2_enrollment_rules.json`。PF0C の統計 policy・598 / 946 は不変。
