# PR4-PF0E-3: record_missed の works_sequence 欠陥の最小修正案（offline 準備のみ・未採用）

状態: 提案。main へは採用しない。commit / push は承認待ち。synthetic fixture のみで作成（production DB・実 ledger / checkpoint / H2 データは未読）。

## 欠陥
`record_missed()` は `Report.works_sequence = after.baseline_sequence`（baseline 固定値、現行 4500）を返す。
`run()` は `r.works_sequence < p.works_sequence`（前回 checkpoint）で STOP する。
library の works_sequence が baseline を超えて checkpoint に記録された後に missed を実行すると、
missed record は ledger に追記された後に STOP し、checkpoint が書かれない（ledger 件数 > checkpoint の record_count になり、以後の run も整合しなくなる）。

## 修正（最小）
- `record_missed(dir, scheduled_at, expected, carried_works_sequence: i64)`: 引数を 1 つ追加。
- 返す値は `carried_works_sequence.max(after.baseline_sequence)`。missed run は DB を開かない（snapshot が無いから missed になる）ので「現在値」は観測できない。直前 checkpoint の値を持ち越すのが正しい。
- `run()` は `record_missed(..., p.works_sequence)` を渡す。
- `run()` の判定ロジック、enroll / init_baseline の経路は無変更。missed では `r.works_sequence == p.works_sequence` となり、減少判定は発火しない（巻き戻しの検出は次の enroll が行う）。

diff: `docs/PR4_PF0E3_RECORD_MISSED_FIX.diff`（+24 / -4、`pr4_pf0e.rs` のみ）。

## テスト
- 追加（先に失敗を確認済み）: `a_missed_run_after_the_works_sequence_grew_past_the_baseline_still_gets_a_checkpoint`
  baseline → work 追加 → enroll（works_sequence が baseline 超え）→ missed。修正前は「works_sequence が前回の checkpoint より減っています」で STOP。修正後は checkpoint_seq=3 が書かれ、works_sequence は直前値を持ち越し、checkpoint の record_count が ledger と一致。
- `cargo test --lib pr4_pf0e`: 14 passed / 0 failed / 1 ignored（既存 13 件 + 新規 1 件。manual は ignore のまま）。

## tool sha256
- 現行 frozen: `183e0c73c14c20afd9ed34da33586f9599f96d1fff95c5c9a79d0b78855e3213`
- 修正後: `36f869f6d197615a610a50e1a164f1492eae06dd1c0eaad778c5311890518d08`
- `tool_sha256()` は `include_str!("pr4_pf0e.rs")` なのでテストコードも含む。**ファイルを 1 文字でも変えると再び変わる**。採用時はレビュー後の最終ファイルで再計算すること。

## 影響範囲（DAILY freeze amendment が要る項目）
1. tool sha256 の変更（checkpoint の `enrollment_tool_sha256` が途中から変わる。チェーン自体は prev sha で繋がるので壊れないが、amendment で新旧 sha と切替 run を明記する必要あり）。
2. tool_head / SOURCE_HEAD（現 HEAD 0aca746 からの変更）。
3. 凍結契約（docs の ledger contract）に「missed の works_sequence は直前 checkpoint の持ち越し」を追記。
4. `record_missed` は pub のため署名変更（呼び出し元は `run()` のみ、repo 内で確認済み）。

## daily_enroll.py への影響
`daily_enroll.py` は本 worktree の追跡ファイルに存在せず（main checkout 側の運用スクリプトのはずで、未読）、確認できていない。
repo 内の呼び出し経路は `manual_pr4_pf0e_run`（環境変数 ACTION=missed）→ `run()` のみで、署名変更は `run()` の内側に閉じる。env 変数・CLI・report の JSON 形（キー）は不変なので、スクリプトが `cargo test` 経由でこの ignored test を呼ぶ形なら変更不要の見込み。ただし tool sha を固定値で比較している場合は更新が必要。Run #2 の監査で実物を確認してほしい。

## 残る限界
- missed の works_sequence は観測値ではなく持ち越し値。missed の間の巻き戻しは次の enroll で初めて検出される（従来も missed は DB を見ないので同じ）。
- すでに欠陥で壊れた状態（ledger に missed があるのに checkpoint が無い）の修復は対象外。修正前の DAILY では staging コピーで防がれている前提。復旧手順は別途設計が必要。
- 修正は main の running DAILY には未適用。適用は amendment と Run #2 完了後の正式監査待ち。
