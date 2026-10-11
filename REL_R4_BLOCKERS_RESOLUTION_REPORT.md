# REL-R4 Blockers Resolution レポート

**判定: HOLD（R4c の実機 GUI 再確認待ち）** — commit/push/merge なし。production APPDATA/DB・H2・main 未接触。R4b の hook 方式は撤去し custom template に置換済み。

## C. F12 / DevTools（R4d）
R4c 実機: F12 で Edge の「開発者ツールを開きますか？」ダイアログが出た（他項目は PASS）。
**source 確認（read-only、Cargo.lock のバージョン: tauri 2.10.3 / tauri-runtime-wry 2.10.1 / wry 0.54.4 / tauri-utils 2.8.3）**
- Cargo feature `devtools` は `open_devtools()` API 等のコンパイルを制御するだけ。
- `tauri-runtime-wry` は `with_devtools(webview_attributes.devtools.unwrap_or(true))` を wry に渡し、wry(WebView2) は `settings.SetAreDevToolsEnabled(attributes.devtools)` を呼ぶ。**config 未指定だと release でも true** → feature を外しただけでは F12 / 検査が WebView2 側で有効のまま（今回の原因）。
- window config `devtools` は `tauri-runtime` の WebviewAttributes（`.devtools(config.devtools)`）経由で同じ経路に入るため、`false` 明示が効く。
**修正**: `tauri.conf.json` の main window に `"devtools": false` を明示。Cargo feature は未有効のまま（`cargo tree -e features -i tauri` にも devtools 0 件）。runtime code・identifier・NSIS template・R4c の version/NSIS 修正は無変更。
**契約テスト**: `main_window_devtools_explicitly_disabled`（全 window の devtools == false）を追加。既存 `devtools_feature_not_forced_in_manifest` も維持。
**再検証**: `cargo test --locked` 635 passed / 0 failed / 14 ignored、`release_guards` 8 passed。path-remap release build / NSIS / MSI 成功、version 1.0.0、developer path・secret 0（exe/NSIS/MSI/展開物）、生成 installer.nsi に delete-app-data 関連 0。
| 成果物 (R4d) | size | SHA-256 |
|---|---|---|
| exe | 21,505,024 | `D6CFA99D1D545BA820166DCAFB587BD8AACCCCEAF6FE6F5CDFD5AAC7763FAAA3` |
| NSIS | 5,346,337 | `54A73176874B54977AE205A658051586A132AED5AD047A81C110C11AF367A76A` |
| MSI | 7,626,752 | `FB75B3B76B5895E461309E5CC84F54F1D691B282348325F367F93439BFFFAFFF` |
Staging: `CineMantisRC-Staging\CineMantis_1.0.0_R4d_x64-setup.exe`（SHA 一致）。R4-SMOKE.txt は「F12 で何も出ない」のみに更新。
実機で効かなければ HOLD（代替案: 設定のみでは不足の場合、webview 側の追加設定が必要＝runtime 変更なので別判断）。
git diff --stat: 既存 12 ファイル（507+/103-）。未 commit。

※以下 A/B のビルド成果物（SHA 等）は R4c 時点の記録で、最新は上表の R4d。

## A. version 回帰ガード
`ui_source_has_no_hardcoded_old_app_version` を追加済み（`src/**/*.ts(x)` に `0.1.0` が無いこと）。

## B. NSIS uninstall で APPDATA 消失（最終方式: custom template）
**調査（Tauri CLI 2.10.1 の生成 installer.nsi）**: 標準テンプレは uninstall 確認ページに「アプリデータを削除」チェックボックスを追加し（既定未チェック）、チェック時のみ `$APPDATA\${BUNDLEID}` と `$LOCALAPPDATA\${BUNDLEID}` を再帰削除する。他に app data を消す処理はなく、silent/passive は確認ページを飛ばすため削除されない。実測消失の原因は確定できず（チェック操作等の可能性）。

**方針**: 「押しても無効なチェックボックス」は不採用。標準テンプレから当該機能そのものを除去。

**テンプレート由来（固定）**
- Tauri CLI: 2.10.1
- upstream: github.com/tauri-apps/tauri, tag `tauri-cli-v2.10.1`, `crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi`（raw.githubusercontent.com から取得、31,256 B / 953 行）
- upstream SHA-256: `FE22026F68BDB3292FAB376756035496CE0A35E3D580E06EBAA6A28295916EB3`
- patched（`apps/desktop/src-tauri/installer-template.nsi`）SHA-256: `91792F3F61FCFC87EC25D120A05F92A495389B647B70EDE6E800D242F5B5ACC8`
- patch（user-data preservation のみ）:
  1. ヘッダにコメント 5 行追加
  2. uninstall 確認ページの delete-app-data 変数・`un.ConfirmShow`・`un.ConfirmLeave`（上流 401–437 行）を削除（確認ページ自体は維持）
  3. Uninstall セクションの「チェック時に registry とアプリデータを再帰削除」ブロック（上流 844–860 行）を削除
  4. 他は上流のまま
- 旧 `installer-hooks.nsh` は削除。`tauri.conf.json` の指定は `bundle.windows.nsis.template` のみ（二重設定なし）。identifier（`dev.cinemantis.app`）・app runtime code は無変更。データ削除機能は未実装（将来はアプリ内の明示操作として別 Gate）。
- 生成された installer.nsi に `DeleteAppData` / `CheckBox` / `$APPDATA` 参照は 0 件。

**再発防止（contract test `nsis_uninstall_preserves_app_data`）**: template 指定、installerHooks 不在、hook ファイル不在、identifier 不変、template に `deleteappdata` / `checkbox` / 再帰 rmdir（トークン単位。`/REBOOTOK` は許容）が存在しないこと、`Section Uninstall` の存在。

**自動 lifecycle テスト**: app data dir を隔離できず、実 install/uninstall は production APPDATA に影響するため未実施 → 実機 GUI 確認（R4-SMOKE.txt）。

## 検証
- `cargo test --locked`: **634 passed / 0 failed / 14 ignored**、`release_guards` 7 passed
- frontend build: tauri build の beforeBuildCommand で PASS
- R2A 方式 path-remap（subst R: + RUSTFLAGS、build 後に解除）で release build / NSIS / MSI 成功、version 1.0.0
- developer path（`C:\Users\tyama\`, `tyama`, `worktrees`, `.claude`, `rel-r4`）/ secret: exe・NSIS・MSI・展開物すべて 0

| 成果物 | size | SHA-256 |
|---|---|---|
| exe | 21,505,024 | `CBB1C9898D204AFE30747CCBAB37BCEEE1DC60D7C5F06099610ABE018EA51B6C` |
| NSIS | 5,344,722 | `2E4EA6DC0B2CFED28F8C3D434F3B4808844B12AAA9A3B387465755639ED2E568` |
| MSI | 7,626,752 | `A9C0BCCCA4F1A4EE4873E801DE2628AEDA48CF827634CA3E12AD22DB01C7B3E4` |

Staging: `C:\Users\Public\CineMantisRC-Staging\CineMantis_1.0.0_R4c_x64-setup.exe`（SHA 一致）。旧版（R4b / R4 / 無印）残置。R4-SMOKE.txt 更新済み。MSI 実機 install は未実施。

## git
`git diff --stat`: 既存 12 ファイル変更（505+/102-、`tauri.conf.json` に template 指定を含む）。未追跡: `installer-template.nsi`、`release_guards.rs`（テスト追加）、本レポート・前回レポート、R1ABC 由来の新規 8 ファイル。未 commit。
