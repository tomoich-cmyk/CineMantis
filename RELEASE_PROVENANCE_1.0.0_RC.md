# RELEASE PROVENANCE 1.0.0 RC（人間向け要約。正本は `RELEASE_PROVENANCE_1.0.0_RC.json`）

| 項目 | 値 |
|---|---|
| app version | 1.0.0（`Cargo.toml` が唯一の権威） |
| baseline | `69d150d633ad1097319e27545962d989a92aebd9`／source commit `e4f173e956b17f73b7a011c03727a0a991ffe444`（親 = baseline、21 ファイル。commit 後 `cargo test --locked` 635 passed / 0 failed / 14 ignored、release_guards 8/8。未 push・未 merge） |
| 正本 worktree | `rel-r4-version-fix`（branch `claude/rel-r4-version-fix`） |
| テスト | R1ABC final 631 passed / 0 failed / 14 ignored → R4d final 635 / 0 / 14 |
| Tauri CLI | 2.10.1 |
| 解決済み crate | tauri 2.10.3 / tauri-runtime 2.10.1 / tauri-runtime-wry 2.10.1 / wry 0.54.4 / tauri-utils 2.8.3 |
| toolchain | rustc/cargo 1.98.1、Node v24.14.0、pnpm 9.0.0、target `x86_64-pc-windows-msvc`（x64） |
| Cargo.lock SHA-256 | `9567eb62ca99485b3fd96a322d77ca2022c4c33baa4f8da361f0ef1d5d108341` |
| NSIS template upstream | tauri-apps/tauri, tag `tauri-cli-v2.10.1`, `installer.nsi`、SHA `FE22026F…16EB3` |
| NSIS template patched | `installer-template.nsi` SHA `91792F3F61FCFC87EC25D120A05F92A495389B647B70EDE6E800D242F5B5ACC8` |
| build 手順 | R2A path-remap（`subst R:` + `RUSTFLAGS --remap-path-prefix` を汎用→具体順、build 後に subst 解除） |
| 署名 | unsigned |
| GUI smoke | PASS（R4d。正本: `REL_R4D_GUI_SMOKE_RESULT.md`、Human 実機操作・スクリーンショット報告を監査記録として採用。R5 が直接観察したものではない） |

## 最終成果物（R4d。再計算済み）
| 成果物 | size (B) | SHA-256 |
|---|---:|---|
| `cinemantis.exe` | 21,505,024 | `D6CFA99D1D545BA820166DCAFB587BD8AACCCCEAF6FE6F5CDFD5AAC7763FAAA3` |
| NSIS `CineMantis_1.0.0_x64-setup.exe` | 5,346,337 | `54A73176874B54977AE205A658051586A132AED5AD047A81C110C11AF367A76A` |
| MSI `CineMantis_1.0.0_x64_en-US.msi` | 7,626,752 | `FB75B3B76B5895E461309E5CC84F54F1D691B282348325F367F93439BFFFAFFF` |
| staging `CineMantis_1.0.0_R4d_x64-setup.exe` | 5,346,337 | NSIS と同一 |

R2A / R4 / R4b / R4c の成果物は **superseded**（出荷しない）。SHA は JSON の `superseded_artifacts_do_not_ship`。
