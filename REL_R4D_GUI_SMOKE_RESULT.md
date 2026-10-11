# REL-R4d 実機 GUI smoke 結果（監査記録）

**種別: Human-observed manual smoke。** 下記の確認項目以外については何も主張しない。

## 条件
- **証拠の出所**: 下記の画面確認（About / Settings / Series / Persons / Awards-Festivals / Source Management / Backup / Audit Report、invalid TMDb key の 401 表示、uninstall UI、APPDATA 保持、reinstall、F12）は、Human による実機操作・スクリーンショット報告を監査記録として採用したものである。**R5 セッションが GUI を直接観察したものではない。**
- 対象 artifact: `CineMantis_1.0.0_R4d_x64-setup.exe`
- 実施者: Human（手動操作・目視）
- テスト環境: 専用 Windows ユーザー `CineMantisRC`
- **production の `tyama` ユーザーの APPDATA / DB はテスト対象ではない。**
- この文書化作業ではソースコードを変更していない（commit / push / merge なし）。

## 結果
| # | 確認項目 | 結果 |
|---|---|---|
| 1 | About の version = 1.0.0 | PASS |
| 2 | Settings の version = 1.0.0 | PASS |
| 3 | Series 画面 | PASS |
| 4 | Persons 画面 | PASS |
| 5 | Awards/Festivals 画面 | PASS |
| 6 | Source Management 画面 | PASS |
| 7 | Backup 画面 | PASS |
| 8 | Audit Report 画面 | PASS |
| 9 | 無効な TMDb テストキー: HTTP 401 が表示され、アプリはクラッシュしなかった | PASS |
| 10 | NSIS アンインストール UI に delete-app-data チェックボックスがない | PASS |
| 11 | アンインストールは正常に完了した | PASS |
| 12 | アンインストール後も `%APPDATA%\dev.cinemantis.app` が保持された | PASS |
| 13 | `cinemantis.db` が残っていた | PASS |
| 14 | 再インストールが完了し、アプリが起動した | PASS |
| 15 | CineMantis にフォーカスした状態の F12 で DevTools UI が出なかった | PASS |

## F12 に関する観察
Edge の F12 確認ダイアログは、フォーカスが CineMantis の**外**にあるときにのみ観測された。これは CineMantis の WebView の DevTools とは扱わない。

## 範囲外（本確認では主張しない）
MSI の実機 install、起動失敗ダイアログ・`startup.log` の実機確認、アプリ内 restore フロー、署名。
