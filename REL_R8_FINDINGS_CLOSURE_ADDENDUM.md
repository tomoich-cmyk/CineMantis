# REL-R8 Findings Closure Addendum

REL-R8（1.0.0 Release Ancestry Audit）は B = HOLD だった。本書は、その未決事項が閉じたことだけを記録する。

| # | R8 の指摘 | 結論 | 根拠 |
|---|---|---|---|
| H1 | 「GTレビュー」タブが production UI に出る | **REL-R9 で解消** | production UI のナビゲーションから除去（backend と Tauri command 8 個は保持）。契約テスト `audit_screen_has_no_gt_review_tab`、`ground_truth_review_panel_is_unreachable_from_ui`。実機: `REL_R9_GUI_SMOKE_RESULT.md` |
| H2 | migration 015 が未配線 | **1.0.0 では DEFER を承認** | 別 Gate。配線時は `foreign_keys=ON` を実行直前に確認し、OFF なら STOP |
| H3 | 休眠 Jev コード等の同梱 | **ship disabled を承認** | code present / runtime disabled by default。環境変数 3 つが揃わない限り動かず、UI 導線なし。Jev を製品に存在しないものとは扱わない |
| — | NAS 既定値 `\\TYNAS\home\Movie`（作者環境の値） | **REL-R9 で解消** | 既定値を廃止。未設定・空白・相対パスは、ファイル操作の前に fail-closed。`TYNAS` は source・生成物とも 0 件 |
| — | debug 用「API テスト」ボタン | accepted, non-blocking | 変更しない |
| — | release profile（LTO・strip なし） | accepted, non-blocking | 変更しない（REL-R2 で履歴確認の課題として残す） |
| — | CSP の緩和 | accepted, non-blocking | 変更しない |

## 結論
R8 の B / HOLD の未決事項（H1、H2、H3、NAS 既定値）は、上記のとおり閉じた。114 コミットの ancestry について、C（出してはいけない内容）に当たるものは確認されていない。

## 残る事項（本書では閉じない）
- main への merge は未承認（main は `88c54fc` のまま。RC は `feat/awards-festivals` 先頭の `69d150d` の上にある）。
- NAS 未設定の警告表示タイミング（UX、LOW、1.0.0 後の改善）。
- 015 の採否、FK 6 行の cleanup は、別 Gate。
