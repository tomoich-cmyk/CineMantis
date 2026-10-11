# CineMantis 1.0.0 RC 既知の問題

いずれも 1.0.0 RC のブロッカーではない（判定根拠は各項）。

## A. 既存データの FK 違反 6 行（分類 LOW）
- 対象: `work_award_match_candidates` の 6 行（372 行中）。参照先の `award_import_items` 行（id 910, 1082, 1088, 1204, 1300, 1422）が存在しない。
- **移行（migration）由来: NO。** 1.0.0 の更新前（R3 のコピー検証 before 状態）から存在し、更新後も 6 行のまま。**新規違反 0**。
- 影響: 画面・検索・照合・起動のどこからも到達されない staging データ。クラッシュ要因なし（R3A の読み取り経路調査）。1.0.0 の起動時チェックは「新規の違反のみ失敗」とする設計のため、この 6 行は許容される。
- 原因: 「`foreign_keys` が OFF の外部接続（手動 SQL 等）での削除」が最有力の **probable cause（状況証拠による推定で、未確定）**。実行ログは未確認。
- 対応: 将来の cleanup 候補（別 Gate）。修復 SQL の案は R3A レポートにあるが **実行していない**。リリース前の対応は不要。

## B. 015_award_duplicate_cleanup.sql は未配線・未適用（別 Gate）
- 1.0.0 には**組み込まれておらず、適用されていない**（`UNWIRED_MIGRATION_FILES`）。この RC は 015 を解決済みとはしない。
- 015 は上記 A の 6 行を解消しない（R3A）。
- **将来配線する場合は `foreign_keys=ON` での実行が必須。** OFF の接続で実行すると、現行データでは新たな orphan が 2 行増える見積り（R3A、SELECT による試算）。配線前に別 Gate で再確認すること。

## C. 署名なし・SmartScreen 警告
- インストーラ・exe とも**コード署名なし**。初回実行時に Windows SmartScreen の警告が表示される場合がある。
- 発行元は `cinemantis`（`Cargo.toml` の authors 未設定のため）。公開名の決定は未実施。
- 署名は 1.0.0 RC の範囲外。

## D. MSI は build 成功、管理者権限での実機 install は未実施
- MSI は生成済み（SHA は provenance 参照）。内容は cab 展開で exe 1 本と確認したのみ。
- 実機の install / uninstall は管理者権限が必要なため未実施。**推奨配布は NSIS（currentUser 導入、R4d 実機確認済み）。** MSI を配布する場合は別途検証。

## E. startup.log の日本語文字化け（**未検証**）
- 事実（コードで確認）: `startup.log` はアプリが UTF-8（BOM なし）のバイト列で書く（`startup.rs` の `write_all(line.as_bytes())`）。
- 観測: 日本語を含む UTF-8 ファイルを **Windows PowerShell 5.1 の既定（`Get-Content`、ANSI 解釈）で読むと文字化けして見える**ことは、このセッションでソースファイル（同じく UTF-8）を読んだ際に再現した。これは**読み出し側の問題**と考えられる。
- 未確認: 実機で生成された `startup.log` の中身、アプリ内ダイアログ（MessageBox）の表示は、どのレポートにも検証記録がない（R1B は「ダイアログは実機未確認」）。したがって**「アプリ自体が文字化けしたログを書く」とも「問題ない」とも断定しない。**
- 回避: ログは UTF-8 対応のエディタ、または `Get-Content -Encoding UTF8` で開く。
- 対応候補（別 Gate・コード変更）: ログ先頭に BOM を付ける、または実機で失敗ケースを起こして確認する。

## F. PR4 は未完了（リリースをブロックしない）
- PR4 の H2 検証と FINAL 判定は**未完了**（FINAL = NOT_RUN）。
- 1.0.0 は**現行の production matcher のまま**リリースできる。今回 matcher / services には変更がない。
- Jev は disabled のまま。本リリースに PR4 の昇格は含まれない。

## G. その他（RC 判断の参考）
- `get_last_restore_result` は用意されているが、画面からの表示導線は未実装。復元の失敗はダイアログとログで通知される。
- 実署名・MSI 実機に加え、**アプリ内 restore フローの GUI 経由での確認**と**起動失敗ダイアログの実機確認**は未実施。
- identifier `dev.cinemantis.app` の末尾 `.app` が macOS 向けの警告を出す（Windows 配布に影響なし。変更はデータフォルダ移行を伴うため行わない）。
- release profile は `opt-level = 2` のみ（LTO・strip なし）。exe は約 20.5 MiB。
- 実データコピーによる更新テスト（R3）は debug ビルドのテストハーネスで実施した。最終 R4d の exe を本番相当データのコピーで起動する確認は行っていない（R4d の GUI 確認は別 Windows ユーザー `CineMantisRC` の新規プロファイル。`R4-SMOKE.txt` より）。
