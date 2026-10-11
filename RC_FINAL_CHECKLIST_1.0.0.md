# CineMantis 1.0.0 RC 最終チェックリスト

対象: 最終成果物 R4d（exe `D6CFA99D…FAAA3` / NSIS `54A73176…A76A` / MSI `FB75B3B7…FFF`。詳細は RELEASE_PROVENANCE_1.0.0_RC.json）

| # | 項目 | 判定 | 根拠 |
|---|---|---|---|
| 1 | R1ABC 統合 | **PASS** | R1ABC レポート: 統合テスト 16/16、`cargo test --locked` 631 passed / 0 failed / 14 ignored（R2A で 631 を再確認） |
| 2 | installer | **PASS**（NSIS）/ MSI は build のみ | R2/R2A/R4d: NSIS silent install・uninstall・reinstall、R4d は Human 実機報告。MSI 実機は未実施（既知の問題 D） |
| 3 | path privacy | **PASS** | R2A 方式 path-remap。R4d の exe/NSIS/MSI/展開物で `C:\Users\tyama\`・`tyama`・`worktrees`・`.claude`・`rel-r4` = 0、secret = 0 |
| 4 | real DB copy migration | **PASS** | R3: user_version 0→25、行変更 0、新規 FK 違反 0、再起動で冪等、本番 DB 不変（debug ハーネス） |
| 5 | FK audit | **LOW** | R3A: 既存 6 行、migration 由来でない、到達経路なし、ブロッカーでない |
| 6 | GUI smoke | **PASS**（R4d） | 正本証跡: `REL_R4D_GUI_SMOKE_RESULT.md`（Human の実機操作・スクリーンショット報告を監査記録として採用。R5 が直接観察したものではない。専用ユーザー CineMantisRC）。`rel-r4-efae4c` のレポートは旧 HOLD 版で superseded |
| 7 | uninstall が APPDATA を保持 | **PASS** | R4d Human 実機報告。delete-app-data チェックボックス不在、template を contract test で固定 |
| 8 | reinstall 後もデータ保持 | **PASS** | R4d Human 実機報告: reinstall 後に起動 |
| 9 | version 一貫性 | **PASS** | exe FileVersion=ProductVersion=1.0.0、About / Settings とも `getVersion()`、UI に `0.1.0` なし（回帰テストあり） |
| 10 | devtools | **PASS** | Cargo feature 無効 + window `devtools:false` 明示、契約テスト、R4d Human 実機報告で F12 無効 |
| 11 | release notes | **完了** | RELEASE_NOTES_1.0.0_RC.md |
| 12 | rollback instructions | **完了** | ROLLBACK_1.0.0_RC.md |
| 13 | provenance | **完了** | RELEASE_PROVENANCE_1.0.0_RC.json（SHA はファイルから再計算し R4d 報告値と一致） |
| 14 | known issues | **完了** | RC_KNOWN_ISSUES_1.0.0.md（A〜F + G） |

注:
- GUI smoke の PASS は `REL_R4D_GUI_SMOKE_RESULT.md`（Human-observed manual smoke）に記録済み。
- テスト 635 件（R4d）は R4 レポートの値。R5 では再実行していない（コード変更なしのため）。

## 最終判定
**READY FOR RC**

根拠: ① 14 項目に FAIL / HOLD なし（MSI 実機と署名は既知の制限として文書化）。② 最終成果物 R4d の SHA を実ファイルで照合済み。③ データ保全（restore・migration・uninstall）が自動テストと実機で確認済み。④ matcher / PR4 / Jev / 015 に変更なし。
source commit: `e4f173e956b17f73b7a011c03727a0a991ffe444`（RC source / tests / installer template、21 ファイル。commit 後 `cargo test --locked` = 635 passed / 0 failed / 14 ignored、release_guards 8/8）。push / merge は未実施。ビルドを再現した場合は SHA を本チェックリストと照合すること。
