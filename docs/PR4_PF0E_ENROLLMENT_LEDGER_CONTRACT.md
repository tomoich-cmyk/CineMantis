# PR4-PF0E: H2 enrollment ledger（offline build/test・未 commit・未実行）

状態: ツールと契約の実装・テストまで。**最初の登録は未実施。T0 は未開始。** 実装: `apps/desktop/src-tauri/src/services/pr4_pf0e.rs`（test ビルドだけ・手動専用）。

## 運用の流れ
1. 本 gate を監査 → commit / push。
2. recovery 済みの snapshot（アプリ終了・production は byte-copy → copy 側で recovery → immutable。PF0A と同じ手順）を作る。
3. `ACTION=init_baseline`: その時点で存在する全 work を baseline に固定する。**この run の `observed_at` が T0。** 以後 baseline の work は永久に登録しない。
4. `ACTION=enroll`（月次など）: baseline にも ledger にも無い work のうち、`evidence_class = live` のものを work id 順に登録する。前回公表した `(record_count, head)` が必須。
5. 946 件に達したら `close` record を書く。以後に流入した作品は今回の FINAL に加えない。598 は count milestone で、停止条件ではない。

## 規則（PF0D 契約の実装）
- selection に使うのは `evidence_class = live` と「baseline / ledger に未登録」だけ。strong / manual ラベル・matcher 出力・candidate・GT の表は読まない（ソース走査テストで固定）。
- 登録した unit は外さない。ledger は追記のみ。production DB には何も書かない（immutable snapshot を read-only で開き、run の前後で snapshot の SHA が不変であることをテスト）。
- population の判定と input の capture は同じ read transaction（同じ immutable snapshot）で行う。
- 1 run 内の登録順は work id 順（裁量なし）。baseline 前後の境界は `created_at` ではなく ID の集合で決める。
- 既知の限界: run の間に作成されて改名・削除された work は観測されない（観測された時点で非 live なら登録しない）。月次より細かい頻度にすれば減らせる。結果と無関係な事由だが、頻度は T0 前に固定して運用する。

## 入力の固定（PF0E-0 の承認事項）
- `matcher_input_payload`（canonical JSON、`pr4-h2-input-payload-1` / `pr4-canonical-json-1`）: `title_guess`、ファイルごと（`part_no, files.id` 順）の `original_file_name / original_rel_path / original_captured / renamed_by_app / container_tags_json（DB 文字列の literal）/ tags_provenance / tags_provider_hint / source_media_kind`、および `file_path` 依存値（`effective_provenance / effective_provider_hint / episode_id_marks_movie`）。**`file_path` 自体と絶対パスは保存しない。** hash = `matcher_input_sha256`。payload は `payloads/<sha>.json`（audit 側）。
- `enrollment_context`: `evidence_class_at_enrollment / first_seen_at / enrollment_rule_version / enrollment_tool_sha256 / payload・canonicalization version / local・embedded・decision_evidence_sha256`。hash = `enrollment_context_sha256`。evidence hash は audit aid で、将来 parser が変わって不一致になっても membership は外さず、provenance として記録する。
- canonicalization: UTF-8、キーはバイト順、空白なし、整数のみ（浮動小数は拒否）、null と欠落は区別（欠落させない）、配列は順序固定、JSON-in-JSON は literal 文字列として保存。
- file_path 依存は、コード上 `episode_id_marks_movie`（判定に入る）と `provenance`（record only）だけ。**別の隠れた path 依存が見つかったら STOP。**

## record
`{seq, kind: baseline|enroll|close, evaluation_unit_key, first_seen_at, matcher_input_sha256, enrollment_context_sha256, enrollment_context, prev_commitment, record_commitment}`。
`record_commitment = SHA256(canonical(record without commitment) + "\n" + previous_commitment)`、先頭の previous は 64 個の `0`。外に出すのは `ledger_head_sha256` と `record_count`（と件数）だけ。key 一覧は報告に出さない。

## 検証したこと（テスト 9 件）
canonical JSON / payload から matcher の evidence が `capture` と完全一致 / file_path だけを変えても固定 payload は不変（かつ生の再取得は変わる＝依存の実在）/ T0 前は登録されず、新規 live は 1 回だけ登録・再走査で追加なし・非 live は登録されない / strong・手動訂正・locked があっても同じ登録・同じ head、登録後の strong・訂正でも所属不変 / record の改変・中間削除・並べ替え・payload の改変を検出、末尾削除は公表済み (count, head) で検出 / cap で close し、以後は追加しない / run 前後で snapshot 不変・report は件数のみ・enroll は (count, head) 必須 / ソースがラベル・verdict・candidate・GT を読まず、DB に書かない。

## Amendment（pre-T0 監査 HOLD への対応。上の記述より優先する）
1. **work id の非再利用**: `works.id` は `INTEGER PRIMARY KEY AUTOINCREMENT`（`packages/db/migrations/001_initial.sql`）で、削除後も再利用されない。アプリに `sqlite_sequence` を書く経路は無い（`DELETE FROM works` は `commands/source.rs`・`commands/work.rs` のみ）。よって baseline を work_id 集合で定義してよい。前提が崩れる場合（backup 復元など）に備え、baseline に `works_sequence`（`sqlite_sequence.seq`）を固定し、(a) baseline に無く id ≤ `works_sequence` の work が現れた、(b) `works_sequence` が baseline／前回 checkpoint より減った、(c) 最大 id が `works_sequence` を超えた、のいずれかで STOP する（テストで固定）。
2. **H2 enrollment event の正式な定義**（PF0D の「作成時に登録」を置き換える）: 「T0 の後の最初の scheduled observation で、非 baseline の work が target live population にいると観測された時点」。`first_seen_at` はこの意味。work 作成の瞬間の payload は保証しない。登録後の改名・strong ラベル・手動訂正・削除では membership を変えない。cadence は DAILY。実行できなかった scheduled run は `missed` record として残し、遡及登録はしない。裁量での追加 scan・間引きは禁止。1 run で候補が残り枠を超えるときは work id 昇順で 946 件目まで登録して close する。規則は `docs/pr4_h2_enrollment_rules.json`（この amendment で更新。PF0D 凍結時の SHA は変わる。新しい SHA は checkpoint の `enrollment_rule_sha256` に入る）。
3. **独立した checkpoint chain**: 成功した run ごとに、ledger とは別のディレクトリへ checkpoint を追記する（`record_count / ledger_head_sha256 / works_sequence / enrollment_rule_sha256 / enrollment_tool_sha256 / tool_head / 前 checkpoint の sha`）。次の run は operator が値を渡さず、この chain の最後の checkpoint から (count, head) を読み、ledger と照合する。ledger の末尾を削除しても、checkpoint が覚えている count と合わず STOP。checkpoint の改変・欠落も検出する。checkpoint は ledger のディレクトリの内外を入れ子にできない。report の `checkpoint_sha256`（chain の head）は audit freeze に別途固定する。identity や payload は checkpoint に入れない。

## T0 の開始手順（承認待ち）
1. アプリを完全終了し、production DB / WAL / SHM を fingerprint。2. byte-copy し、コピー側だけで standalone の immutable snapshot を作る。3. production fingerprint が前後で不変なことを確認。4. その snapshot から全 work の baseline identity 集合を固定。5. baseline の count・commitment・snapshot SHA を freeze。6. `init_baseline` を実行し、独立 checkpoint を固定。7. この run の UTC が T0。8. その後にアプリを再開する。

## Rule provenance
```
rule_version = pr4-h2-enrollment-rules-2   (binding enrollment rule; sha256 is recorded in every checkpoint as enrollment_rule_sha256)
supersedes_rule_sha256 = dfc847856fd55e5475e444e027b0995d1dc8bf76b82bfa6a090f190697577b8c   (rules-1, frozen by PF0D in commit 2f537a2; kept in git history)
amendment_reason = replace creation-time enrollment semantics with first-scheduled-observation semantics before T0
```
T0 前の pre-enrollment amendment である。H2 の登録は 1 件も行われていない。
