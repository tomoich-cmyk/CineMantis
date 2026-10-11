# REL-R4 Version Fix ビルドレポート

**判定: PASS**（commit / push / merge なし。production APPDATA/DB・H2・main 未変更、CineMantis 起動・installer 実行なし）

## 1. Diff audit
- worktree `rel-r4-version-fix` / base `69d150d`。変更 12 + 新規 8 = 20 ファイル。
- R1ABC 正本 `rel-r1abc-169c10`（READ ONLY）と SHA-256 で機械比較: **18 ファイル完全一致、2 ファイルのみ差分**。
  1. `SettingsScreen.tsx`: `getVersion()`（`@tauri-apps/api/app`）を `useQuery` で取得し `<span>0.1.0</span>` → `<span>{appVersion}</span>`
  2. `commands/release_guards.rs`: `settings_screen_uses_runtime_app_version` テスト追加
- それ以外の semantic delta: **0**（想定どおり）。

## 2. Version audit
- SettingsScreen に `0.1.0` hardcode なし。About / Settings とも `getVersion()` 由来。
- `src` 配下 ts/tsx に `0.1.0` 0 件、`dist/assets/*.js` に 0 件。
- Cargo `[package] version = "1.0.0"`（唯一の app version）。root / desktop / shared-types の package.json・tauri.conf.json に version なし。
- exe: FileVersion = ProductVersion = **1.0.0**。

## 3. Regression
- `cargo test --locked`: **632 passed / 0 failed / 14 ignored**（親の主張と一致）
- `release_guards`: 5 passed（新規テスト含む）
- frontend build（`corepack pnpm build`）: PASS

## 4. R2A 方式 path-remap release build
- `subst R:`（リポジトリ root）経由、`apps\desktop` から `corepack pnpm tauri build`、`--locked` 相当の Cargo.lock 使用。
- RUSTFLAGS: `.cargo` → `.rustup` → `.cargo\registry\src`（汎用→具体の順）→ repo root → `R:\`。prefix は `$env:USERPROFILE` から動的生成。ソース/設定ファイルへの path 書き込みなし、release profile 未変更。build 後 `subst R: /d` で解除済み。
- exe / NSIS / MSI（および NSIS 展開物・MSI 内 exe）の走査（ASCII・UTF-16LE、大小無視）:
  `C:\Users\tyama\` 0 / `tyama` 0 / `worktrees` 0 / `.claude` 0 / `rel-r4` 0（repo 絶対パス 0）。
  `R:\` の 2 件は exe 内の圧縮データ（バイナリ偶然一致）で path ではない。
- secret パターン（sk-/ghp_/AKIA/PRIVATE KEY/api_key 等）: 0 件。

## 5. 成果物（version 1.0.0）
| 成果物 | size (B) | SHA-256 |
|---|---|---|
| `target/release/cinemantis.exe` | 21,505,024 | `D89DFE1145682E6E1275405D3AFF3AA757E639B9870A56B7B50168C332CB2993` |
| NSIS `CineMantis_1.0.0_x64-setup.exe` | 5,343,705 | `34244BAE658028B6BE338A51FFE8EE08BFD0816E023ADF4E8B8E1D4E1C56821A` |
| MSI `CineMantis_1.0.0_x64_en-US.msi` | 7,626,752 | `664078CC33CABED611DA097FBED78A1BB87ECE92EF9EF78DBD8B9DAF873195A5` |

場所: `apps/desktop/src-tauri/target/release/`（`bundle/nsis`, `bundle/msi`）。MSI の実機 install は未実施（管理者権限、REL-R2 から継続 defer）。

## 6. Staging
- `C:\Users\Public\CineMantisRC-Staging\CineMantis_1.0.0_R4_x64-setup.exe` に配置。コピー元/先 SHA 一致。
- 既存の `CineMantis_1.0.0_x64-setup.exe`（R2A 版 SHA `82BE3BA1…`、Settings が 0.1.0 表示の旧版）は**削除せず残置**。R4 版は別名。
- `R4-SMOKE.txt` を同フォルダに作成（Human GUI 確認項目）。

## 7. Human が次に行う操作
CineMantisRC ユーザーで `CineMantis_1.0.0_R4_x64-setup.exe` をインストールし、`R4-SMOKE.txt` に従って確認: fresh profile 起動 / About・Settings が 1.0.0 / devtools 無効 / 主要画面遷移 / 失敗 UX / 再起動後の永続 / アンインストール。結果を報告。

## 補足
- 作業ツリーは未 commit のまま（禁止事項のため）。`target/`・`dist/` は gitignore 対象。
