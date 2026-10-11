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
