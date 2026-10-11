# CineMantis 1.0.0 RC 最終チェックリスト

対象: 最終成果物 R9（exe `AC608EAC…DBCA` / NSIS `6737768B…15DB3` / MSI `D3C4AC7C…414F`。source commit `0cf98e23d786a75c7173e0aec9058496d8079558`。詳細は RELEASE_PROVENANCE_1.0.0_RC.json）。R4d は superseded。

| # | 項目 | 判定 | 根拠 |
|---|---|---|---|
| 1 | R1ABC 統合 | **PASS** | R1ABC レポート: 統合テスト 16/16、`cargo test --locked` 631 passed / 0 failed / 14 ignored（R2A で 631 を再確認） |
| 2 | installer | **PASS**（NSIS）/ MSI は build のみ | R2/R2A/R4d: NSIS silent install・uninstall・reinstall、R4d は Human 実機報告。MSI 実機は未実施（既知の問題 D） |
| 3 | path privacy | **PASS** | R2A 方式 path-remap。R9 の exe/NSIS/MSI/展開物で `C:\Users\tyama\`・`tyama`・`worktrees`・`.claude`・`rel-r9`・`rel-r4`・`TYNAS` = 0 |
| 4 | real DB copy migration | **PASS** | R3: user_version 0→25、行変更 0、新規 FK 違反 0、再起動で冪等、本番 DB 不変（debug ハーネス） |
| 5 | FK audit | **LOW** | R3A: 既存 6 行、migration 由来でない、到達経路なし、ブロッカーでない |
| 6 | GUI smoke | **PASS**（R9） | 正本証跡: `REL_R9_GUI_SMOKE_RESULT.md`（Human の実機操作・スクリーンショット報告を監査記録として採用。セッションが直接観察したものではない。専用ユーザー CineMantisRC）。R4d の記録 `REL_R4D_GUI_SMOKE_RESULT.md` は履歴。`rel-r4-efae4c` のレポートは旧 HOLD 版で superseded |
| 7 | uninstall が APPDATA を保持 | **PASS**（R4d で実機確認。R9 は同一 template。R9 での再実施は未実施） | R4d Human 実機報告。R9 の NSIS template SHA は R4d と同一（`91792F3F…ACC8`）。delete-app-data チェックボックス不在、template を contract test で固定 |
| 8 | reinstall 後もデータ保持 | **PASS**（R4d で実機確認。R9 での再実施は未実施） | R4d Human 実機報告: reinstall 後に起動 |
| 9 | version 一貫性 | **PASS** | exe FileVersion=ProductVersion=1.0.0、About / Settings とも `getVersion()`、UI に `0.1.0` なし（回帰テストあり） |
| 10 | devtools | **PASS** | Cargo feature 無効 + window `devtools:false` 明示、契約テスト、R9 Human 実機報告で F12 無効（CineMantis にフォーカスした状態） |
| 11 | release notes | **完了** | RELEASE_NOTES_1.0.0_RC.md |
| 12 | rollback instructions | **完了** | ROLLBACK_1.0.0_RC.md |
| 13 | provenance | **完了** | RELEASE_PROVENANCE_1.0.0_RC.json（R9 の SHA はファイルから再計算し R9 報告値と一致） |
| 14 | known issues | **完了** | RC_KNOWN_ISSUES_1.0.0.md（A〜H） |
| 15 | R8 findings（GT レビュー UI / NAS 既定値） | **解消** | R9 で修正。`REL_R8_FINDINGS_CLOSURE_ADDENDUM.md` |

注:
- GUI smoke の PASS は `REL_R9_GUI_SMOKE_RESULT.md`（Human-observed manual smoke）に記録済み。R8 の HOLD 事項の解消は `REL_R8_FINDINGS_CLOSURE_ADDENDUM.md`。
- テスト: R9 の source commit `0cf98e23d786a75c7173e0aec9058496d8079558` で `cargo test --locked` = **643 passed / 0 failed / 14 ignored**、release_guards **11/11**（commit 後の実測）。

## 最終判定
**READY FOR RC**

根拠: ① 15 項目に FAIL / HOLD なし（MSI 実機と署名は既知の制限として文書化）。② 最終成果物 R9 の SHA を実ファイルで照合済み。③ データ保全（restore・migration・uninstall）が自動テストと実機で確認済み（uninstall/reinstall の実機確認は R4d。R9 は同一 template）。④ matcher / PR4 / 015 に変更なし。Jev は code present / runtime disabled by default。
source commit: `0cf98e23d786a75c7173e0aec9058496d8079558`（REL-R9 hygiene fix、8 ファイル。親は RC docs `ecbd04c`、RC source `e4f173e`）。push / merge は未実施。ビルドを再現した場合は SHA を本チェックリストと照合すること。
