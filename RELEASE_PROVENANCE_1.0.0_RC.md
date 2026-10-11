# RELEASE PROVENANCE 1.0.0 RC（人間向け要約。正本は `RELEASE_PROVENANCE_1.0.0_RC.json`）

| 項目 | 値 |
|---|---|
| app version | 1.0.0（`Cargo.toml` が唯一の権威） |
| baseline | `69d150d633ad1097319e27545962d989a92aebd9` |
| source commit（最終成果物 R9 に対応） | `0cf98e23d786a75c7173e0aec9058496d8079558`（REL-R9 hygiene fix、8 ファイル。親 `ecbd04c` = RC docs、その親 `e4f173e` = RC source）。commit 後 `cargo test --locked` 643 passed / 0 failed / 14 ignored、release_guards 11/11。未 push・未 merge |
| 正本 worktree | `rel-r9-817874`（branch `claude/rel-r9-817874`） |
| テスト | R1ABC 631 → R4d 635 → **R9 643 / 0 / 14**（release_guards 11/11） |
| Tauri CLI | 2.10.1 |
| 解決済み crate | tauri 2.10.3 / tauri-runtime 2.10.1 / tauri-runtime-wry 2.10.1 / wry 0.54.4 / tauri-utils 2.8.3 |
| toolchain | rustc/cargo 1.98.1、Node v24.14.0、pnpm 9.0.0、target `x86_64-pc-windows-msvc`（x64） |
| Cargo.lock SHA-256 | `9567eb62ca99485b3fd96a322d77ca2022c4c33baa4f8da361f0ef1d5d108341` |
| NSIS template upstream | tauri-apps/tauri, tag `tauri-cli-v2.10.1`, `installer.nsi`、SHA `FE22026F…16EB3` |
| NSIS template patched | `installer-template.nsi` SHA `91792F3F61FCFC87EC25D120A05F92A495389B647B70EDE6E800D242F5B5ACC8`（R9 でも同一） |
| build 手順 | R2A path-remap（`subst R:` + `RUSTFLAGS --remap-path-prefix` を汎用→具体順、build 後に subst 解除） |
| 署名 | unsigned |
| GUI smoke | PASS（R9。正本: `REL_R9_GUI_SMOKE_RESULT.md`、Human 実機操作・スクリーンショット報告を監査記録として採用。セッションが直接観察したものではない）。R4d の記録 `REL_R4D_GUI_SMOKE_RESULT.md` は履歴 |
| Jev | code present / runtime disabled by default（環境変数3つが揃わない限り動かず、UI 導線なし） |
| GT レビュー | production UI のナビゲーションから除去。backend と Tauri command 8 個は保持 |
| NAS root | 既定値なし。未設定・空白・相対パスは、ファイル操作の前に fail-closed |
| 015 | 未配線・未適用・deferred（別 Gate。配線時は `foreign_keys=ON` 必須） |
| MSI 実機 install | 対象外（not covered） |

## 最終成果物（R9）
| 成果物 | size (B) | SHA-256 |
|---|---:|---|
| `cinemantis.exe` | 21,552,640 | `AC608EACA539DCC825C4B44119EB69CD350153919D1CBCCA92F8C1481F94DBCA` |
| NSIS `CineMantis_1.0.0_x64-setup.exe` | 5,337,569 | `6737768BE8F4644CCAB43FBBB0B59D4EF7FB2FF1F8EE00D81A991B1311015DB3` |
| MSI `CineMantis_1.0.0_x64_en-US.msi` | 7,639,040 | `D3C4AC7C7D1145864F31AEA89D827965EDB2937A8CD91BEEC2B4AC4DC914414F` |
| staging `CineMantis_1.0.0_R9_x64-setup.exe` | 5,337,569 | NSIS と同一 |

走査（exe / NSIS / MSI の生バイトと展開物）: `C:\Users\tyama\` / `tyama` / `worktrees` / `.claude` / `rel-r9` / `rel-r4` / `TYNAS` = 0。

R4d / R4c / R4b / R4 / R2A の成果物は **superseded**（出荷しない）。SHA は JSON の `superseded_artifacts_do_not_ship`（R4d: exe `D6CFA99D…FAAA3` / NSIS `54A73176…A76A` / MSI `FB75B3B7…FFF`）。
