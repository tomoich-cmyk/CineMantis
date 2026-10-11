# DEFERRED_FIXES（PF0I fast-track）

停止理由にしない LOW / 文書・後続 gate の宿題。

| # | 内容 | 持ち越し先 |
|---|---|---|
| D1 | `build_secret_commitment` の導出式を PF0G は定義していない。PF0I は SHA256(build_secret_hex の ASCII) に固定。PF0G2 packager が従う | PF0G2 |
| D2 | unblind bundle の形（`pr4-pf0i-unblind-bundle-1`）は PF0I 定義。PF0G2 が出力する | PF0G2 |
| D3 | `errata.jsonl` の promotion-impacting（evaluation = INVALIDATED）の確認は FINAL runner の責務（adapter は読まない） | PF0I2 / FINAL runner |
| D4 | **確定**: PF0H final manifest SHA = FINAL_MEMBERSHIP_COMMITMENT.json の canonical SHA-256（H2_CLOSE.json の生 SHA は supplemental evidence・binding anchor ではない）。**loader 実装済み**（`load_pf0h_final_membership`） | PF0I loader gate |
| D5 | PF0G pin `machine_freeze_anchor_commitment` と manifest の `machine_freeze_commitment_sha256` の関係（別値の可能性） | PF0G2 |
| D6 | 実 `Store` が出した ledger との E2E 1 本（PF0G1 の test fixture は private） | PF0G2 完成後 |
| D7 | `pr4_blind_gt` は PF0G1 で `pub mod`（製品ビルドに含まれる）。PF0I adapter は cfg(test) のみ。PF0G 側の判断 | PF0G |
| D8 | PF0G2 packager が出す bundle / build_secret_commitment は統合 E2E で PF0I と一致を確認済み（D1・D2・D6 クローズ）。残: 統合 E2E ファイル `pr4_pf0i_e2e_tests.rs` は単体 branch でコンパイルしないので mod.rs 未登録。3 branch（PF0G2・PF0H1・PF0I）が main に入った後に登録する | main 統合時 |
| D5 更新 | READ ONLY 確認済み: PF0G docs / PF0G2 に「pin machine_freeze_anchor_commitment = manifest machine_freeze_commitment_sha256」という記述は無い。別概念として扱う。同一性は運用側で確認（OPEN） | 運用 |

---

# PF0G2 DEFERRED_FIXES（LOW / 文書・命名・後続 gate。Hard STOP ではない）

| # | 内容 | 重大度 | 送り先 |
|---|---|---|---|
| D1 | UI の CSP が `script-src 'unsafe-inline'`（単一 HTML に inline script）。UI を external file + hash/nonce にする | LOW | PF0G3 |
| D2 | 実ブラウザ（built-in browser）での E2E・a11y・日本語文言の目視確認は未実施（API 契約と JS 構文のみ検証）。poster 画像は live fetch 禁止のため未表示 | LOW | PF0G3 |
| D3 | reviewer 用の bin（`Server::start` を呼ぶ main、API key を env から読む、`SystemProcessLister` を使う）は未作成。`Api` / `Server` / `ProcessProbe` は完成 | LOW | PF0G3 |
| D4 | `authoritative_membership_commitment` と PF0H final manifest SHA は **PF0H から受け取る入力**として sealed handoff に載せるだけ（packager は計算しない・形式は 64 hex のみ検査）。PF0H/PF0I 側の定義（何の hash か）が確定したら packager 側で再計算・突合する | LOW（定義待ち） | PF0H/PF0I |
| D5 | `Store` が `Send` でない（PF0G1 の `MainAppProbe` に `Send` 境界が無い）ため、`Server::start` は factory 方式にした。core を変えない制約による | LOW | 不要 |
| D6 | `pins_from_ledger` は events.jsonl の先頭 init event を直接読む（`Store` が pin を公開しないため）。chain 検証は `Store::open` 側で済んでいる前提 | LOW | PF0G1 を変える gate があれば `Store::pins()` |
| D7 | pick_other の TMDB 検証は自動再試行しない（人が再試行）。PF0F2 の retry/backoff policy は machine run 用で reviewer は対象外。timeout 実時間（30s）は未実測 | LOW | — |
| D8 | search の `media_type` は movie / tv のみ（schema の `multi` は使わない）。UI も movie / tv を選ぶ | LOW | — |
| D9 | 946 件の full 工程（fsync 付き全 event）は未実行。package 946 件の build/load/scan のみ実行。E-1 rehearsal は PF0G4 | LOW | PF0G4 |
| D10 | 候補の提示順は token（HMAC）昇順。契約 §3.2 の「shuffle(seed=SHA256(review_unit_id‖build_secret))」と文言は違うが、secret 依存・rank 非依存・決定論の性質は同じ。契約文言の更新は文書のみ | LOW | docs |

## FINAL FREEZE 前に閉じる必須項目（Human 指示で D10 / D4 から格上げ）
- **M1**: 契約 §3.2 の seeded shuffle と実装の候補提示順（token 昇順）の不一致を解消する（実装を契約に合わせるか、契約を実装に合わせて改訂する）。
- **M2**: `build_secret_commitment` は PF0I で `SHA256(ASCII(lowercase_hex(build_secret_bytes)))` と意味まで固定された。packager は現状 64 桁 lowercase hex の形式検査と同式での生成だけなので、PF0I の再導出検証（bundle → 全 unit / token の再計算）と同じ検査を packager 側にも入れて突合する。
- （クローズ）PF0H final manifest SHA の対象は canonical SHA256(FINAL_MEMBERSHIP_COMMITMENT.json) で確定。

- （統合 D8 結果）`pr4_pf0i_e2e_tests.rs` は private fixture を使うため、mod.rs の sibling 登録ではコンパイルしない。`pr4_pf0g2_tests.rs` 末尾の `#[path]` child module として登録（679 passed / 0 failed / 13 ignored）。
- （統合 D5 検証）PF0H1 側に machine freeze 系の値は存在しない。pin `machine_freeze_anchor_commitment`（外部 audit anchor）と PF0G manifest の `machine_freeze_commitment_sha256` は別概念のまま・同一性の記述なし。運用側の確認は OPEN。
- （M1/M2）PF0G2 の FINAL FREEZE 前必須 M1（seeded shuffle 文言）・M2（packager 側の build_secret_commitment 再導出検証）は未クローズ。

- （統合 CLOSED）D5: N/A。`machine_freeze_anchor_commitment`（PF0G freeze-domain pin）は PF0H の authoritative_membership_commitment / final manifest canonical SHA / review seal SHA とは別 commitment domain で、parity 関係は不要。PF0I の binding 正式 3 点は変えない。
- （統合 CLOSED）M1: 候補提示順を seed = SHA256(review_unit_id‖build_secret) の決定論 shuffle に変更（token lexical 順を廃止）。M2: build 時に build_secret から build_secret_commitment を再導出し pin と exact-match、不一致は出力前に fail closed。

## PF0G3（実ブラウザ・CSP・reviewer binary）の結果
- （CLOSED）D1 CSP: `unsafe-inline` を除去（`index.html` / `app.css` / `app.js` に分割、`default-src 'none'` ベース）。実ブラウザで inline / data: / 外部がブロックされることを確認。
- （CLOSED）D2 実ブラウザ E2E・a11y（keyboard・role=radio・aria・375px）: 85/85 PASS（`docs/pf0g3_evidence/`）。poster は仕様どおり表示しない（`img-src 'none'`）。
- （CLOSED）D3 reviewer bin: `examples/pf0g3_reviewer.rs` + `pr4_pf0g3_reviewer.rs`。env のみ・fail closed・実 `SystemProcessLister` を Windows で確認。
| # | 内容 | 重大度 | 送り先 |
|---|---|---|---|
| G3-1 | sealed 後の `/api/next` は 410 ではなく `200 {done:true, unit:null}`（lease も event も作らない。`events.jsonl` byte 同一を確認）。UI の挙動に影響なし | LOW | — |
| G3-2 | built-in browser pane は Chromium 152（Electron）のみで確認。Firefox / Safari は未確認。a11y は DOM・keyboard・aria の自動検査まで（スクリーンリーダー実機は未実施） | LOW | PF0G4 運用確認 |
| G3-3 | reviewer 起動の運用 runbook（env の設定手順・ledger の init 手順・token の受け渡し）は未文書化。binary は init 済み ledger（`Store::open`）を前提とし、init は operator 側 | LOW（docs） | PF0G4 |
| G3-4 | `/app.js` `/app.css` は token なしで配る（`<script src>` は header を付けられないため）。pin 済み・secret なし・Host/Origin 検査あり・loopback のみ。本体 `cinemantis.exe` 稼働中は reviewer 自体が起動しない | LOW（受容） | — |
