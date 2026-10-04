# C5c.5 最終 closeout（記録）

TMDB マッチャーの ground truth（GT）評価 C5c.5 は **FINAL CLOSED**（2026-10-04）。
このファイルは結果の要約だけを残す。GT の中身・候補の題名・人手判断の詳細・画像・監査ディレクトリの内容は、このリポジトリに入れない。

## 範囲と最終状態

- 対象: 開発用 50 task（`c5c5c-redo-01` 〜 `05`）。
- **resolved 44 / 50、deferred 6 / 50**。
- deferred の task ID: **72, 74, 87, 111, 113, 115**。
- 内訳: 初回レビュー 40 + 追加の候補提示 3 + 追加の証拠提示 1 = 44。残り 6 件は、用意した証拠を出しても人が決められなかった。

## 指標（2 系統。混ぜない）

どちらも **resolved かつ TMDB に正解がある GT だけ**を分母にした条件付きの値で、**end-to-end の recall ではない**。deferred 6 件は分母に入らない。

| 指標 | 分母 | @1 | @3 |
|---|---|---|---|
| Legacy conditional recall | 43 | 42/43 = 97.7% | 43/43 = 100% |
| F-inclusive conditional recall（別指標） | 44 | 42/44 = 95.5% | 44/44 = 100% |

旧値は書き換えていない。F 込みの値は、後から解決した 1 件を足して別に計算したもの。

## 正式指標から除外するもの

- 旧 provisional の `c5c5-dev-expand-01`（50 件）。出所が未確定のため、どの指標にも入れない。
- smoke の書き出し（`*_SMOKE_decisions*.INVALID.json`）。
- `c5c5e-h1`（人手で検索語を考える拡張）。**任意の探索的な追加研究であり、本線ではない。** 始めた場合も C5c.5 の指標には混ぜない。

## 凍結した closeout 成果物

監査ディレクトリ（リポジトリ外・コミットしない）`CineMantis-audit\c5c5f\closeout\` にある。

| ファイル | SHA-256 |
|---|---|
| `C5c5_FINAL_CLOSEOUT.json` | `31c30915d3d21db90b06fb40334787ab041e818b5a935a14e5f4c2dde9bd9c8b` |
| `F_CLOSEOUT.json` | `172c797196c7896cd1689fc187a005aaef3b1eb9f88edc7027b1f61150c55ead` |

## 状態のまとめ

- production の割り当て・DB・holdout には触れていない。
- **live の holdout は未開封**のまま。昇格判断は行っていない。
- 次の作業（PR4）の入口は `docs/FOLLOWUPS.md` の「PR4 開始前の状態」を参照。
