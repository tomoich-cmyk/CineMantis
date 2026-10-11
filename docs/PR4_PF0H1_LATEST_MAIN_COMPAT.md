# PR4-PF0H1 latest-main compatibility report

- main H2 worktree HEAD: `69d150d633ad1097319e27545962d989a92aebd9`（`git rev-parse` で確認のみ。main worktree には書いていない）。
- PF0H 出発点: `556a99f`（branch `claude/pr4-pf0h-455c28`）。`69d150d` の直接の子。
- 取り込み: 別 worktree `claude/pr4-pf0h1-e60774`（元 base `88c54fc`、これは 556a99f の祖先）を `556a99f` へ fast-forward。`git merge-base --is-ancestor 69d150d HEAD` = true。履歴書き換え・reset なし。
- 差分範囲: `git diff 69d150d HEAD` は PF0H の 6 ファイル（mod.rs、close / verify / tests の 3 rs、contract、schema）のみ。`pr4_pf0e.rs`・`pr4_pf0b.rs` の差分は 0 行（PF0E の tool / rules 不変）。
- PF0B `membership_commitment` と `MEMBERSHIP_DOMAIN`（`pr4-pf0a-membership-1`）は commit 3d72255 時点と本文同一。
- 再実行: `cargo test --lib pr4_pf0h` = 26 passed / 0 failed / 1 ignored。`cargo test --lib pr4_` = 137 passed / 0 failed / 5 ignored（close / verify 回帰、source scan T45、PF0E verify_ledger 一致 T46 を含む）。
- 注意: main の HEAD が 69d150d より先に進んだ場合は、PF0E の tool / rules に変更が無いことを同じ手順（diff 0 行）で再確認すること。
- 実 H2 のデータには触れていない。
