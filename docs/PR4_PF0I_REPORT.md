# PR4_PF0I_REPORT（PF0I0 final alignment + PF0I1 synthetic 実装）

結論: **freeze candidate 完成。HARD STOP 要因なし。commit / push は Human Gate 待ち。** FINAL = NOT_RUN、holdout は sealed のまま。

## 1. やったこと
1. PF0I0 contract/schema を PF0G authoritative `1a6266d` に 1 回だけ final alignment（r2）。PF0G1 実装コードを READ ONLY で突き合わせ、r1 の食い違いを解消（pin は `pins` object 8 項目 / confirm は identity を持たない / token は `ct_`+20 文字 HMAC / seal・init の形）。
2. 監査決定を反映: build_secret・unblind_map の完全隔離（adapter は seal 後に受け取る）/ `work:<base10 integer>` / PF0H に対する exact-match は 3 値のみ / PF0H 正式 loader は schema 確定後に 1 つ（本 gate 未実装）/ token の再導出検証 / ledger には token のみ・identity は bundle から復元 / 失敗は独自 envelope（PF0B を呼ばない・PF0B に似せない）/ seal 後 GT は不変（errata は別）/ PF0B 無変更。
3. PF0I1（synthetic のみ）: `pr4_pf0i_adapter.rs`（adapt / TrustedGtSet / assemble_final_input / 失敗 envelope）と 33 試験。PF0H1 は未確定のため loader は作らず、`Pf0hLoaded`（2 値）を受ける形にして、PF0G 側で確定済みの部分（bundle 復元・token→identity join・GT 変換・失敗 envelope・凍結 policy との評価結合）を先行実装。
4. evidence / provenance: `docs/PR4_PF0I_FREEZE_CANDIDATE.md`（SHA-256 一覧）。`DEFERRED_FIXES.md`（D1–D7）。

## 2. 検証結果
`cargo test --lib` 613 passed / 0 failed（PF0I 33）。mutation（defer_final→NoMatch）で評価結果が変わることを確認（t23）。凍結 policy で PROMOTE・AUTO×defer_final で INCONCLUSIVE（t20/t21）。ledger 全 event の 1 byte 改ざん・入替・削除・追記・pin 8 項目の差替えを全て検出（t11/t14）。出力に work key・identity・token・自由記述が出ないこと（t28）、adapter が DB/fs/net/env を持たないこと（t29）。

## 3. 差分
`git diff --stat`（追跡）: mod.rs +3。新規: pr4_pf0i_adapter.rs / pr4_pf0i_tests.rs / contract / schema / FREEZE_CANDIDATE / REPORT / DEFERRED_FIXES.md。`pr4_pf0b.rs`・`pr4_blind_gt.rs` は無変更。HEAD は 1a6266d へ fast-forward のみ。

## 4. Human 判断が要る点（DEFERRED_FIXES 参照・いずれも停止理由ではない）
- `build_secret_commitment` の式（SHA256 of hex ASCII）を PF0I で固定した（PF0G は未定義）。PF0G2 が従うことの承認。
- unblind bundle の形を PF0I が定義した（PF0G2 が出力）。
- errata の INVALIDATED 確認は FINAL runner の責務。
- PF0H loader は PF0H schema 確定後。

## 5. 次
承認があれば commit（内容: 上記ファイル。branch `claude/pr4-pf0i-653127`）→ push。その後 PF0H 確定を待って loader を 1 つ実装、PF0G2 と bundle・commitment の整合、実 Store ledger との E2E。
