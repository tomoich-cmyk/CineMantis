# REL-R9 実機 GUI smoke 結果（監査記録）

**種別: Human-observed manual smoke。** 下記の確認項目以外については何も主張しない。

## 条件
- 対象 artifact: `CineMantis_1.0.0_R9_x64-setup.exe`（SHA-256 `6737768BE8F4644CCAB43FBBB0B59D4EF7FB2FF1F8EE00D81A991B1311015DB3`、source commit `0cf98e23d786a75c7173e0aec9058496d8079558`）
- 実施者: Human（手動操作・目視）
- テスト環境: 専用 Windows ユーザー `CineMantisRC`
- **production の `tyama` ユーザーの APPDATA / DB はテスト対象ではない。**
- **証拠の出所**: 下記の画面確認は、Human による実機操作・スクリーンショット報告を監査記録として採用したものである。**どのセッションも GUI を直接観察していない。**
- この文書化作業ではソースコードを変更していない。

## 結果
| # | 確認項目 | 結果 |
|---|---|---|
| 1 | 監査レポート画面に「GTレビュー」タブがなく、「重複候補」「整合性チェック」の2タブだけ | PASS |
| 2 | 設定画面の NAS 振り分け先が未設定で、「保存先を設定してください」が表示される（placeholder は `\\server\share\Movie`） | PASS |
| 3 | NAS 振り分け画面で、空のテスト用フォルダを選択した後に、NAS 未設定の警告が表示される | PASS |
| 4 | NAS 振り分け画面の実行ボタンが押せない（`disabled`） | PASS |
| 5 | About の version = 1.0.0 | PASS |
| 6 | Settings の version = 1.0.0 | PASS |
| 7 | CineMantis にフォーカスした状態の F12 で DevTools UI が出ない | PASS |

## 観察事項（UX、LOW。1.0.0 後の改善）
- 取り込み元フォルダを選択する**前**は、NAS 振り分け画面に「保存先を設定してください」の警告が表示されない（実行ボタンは、選択済みの作品が 0 件のため `disabled`）。警告は、フォルダを選んだ後に表示される。安全性には影響しない（backend が未設定時に fail-closed）。

## 範囲外（本確認では主張しない）
- uninstall / reinstall でのデータ保持（R4d で実機確認済み。R9 は同一の NSIS template だが、R9 での再実施はしていない）
- MSI の実機 install、起動失敗ダイアログ・`startup.log` の実機確認、アプリ内 restore フロー、署名
- 「NAS へ移動」ボタンの実行（押していない）
