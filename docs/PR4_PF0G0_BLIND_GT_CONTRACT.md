# PR4-PF0G0: H2 FINAL 用 Blind Human GT Reviewer 契約（設計のみ・未実装）

状態: **PF0G1 で凍結済み（実装 `pr4_blind_gt.rs` は synthetic のみ。§17 が §15・§16 の旧記述に優先する）。H2 の実データ・H1・machine output・Jev・production DB は一切読んでいない。** 根拠にした既存資産は `gt_review.rs` / `gt_operator.rs` / `pr4_gt_semantics.json` / PF0C・PF0D・PF0E の docs だけ。
機械可読 schema 案: `docs/pr4_pf0g0_blind_gt_schema.json`（本書 §3 の元）。
branch: `claude/pr4-pf0g-171907`（0aca746 起点）。main checkout は未接触。

---

## 0. 結論（要点）

1. **既存 C5c.5 の UI/API はそのまま流用しない。** 意味（confirm / pick_other / none / defer）は完全に維持するが、入口が blind 条件を満たさない（§1.3）。
2. 構造は **「一方向の packager → 封印された reviewer artifact → 機械情報を知らない reviewer プロセス → audit 側 append-only ledger」**。reviewer プロセスは production DB・machine freeze・PF0E ledger の鍵一覧のどれにも到達できない。
3. **GT は production DB に書かない。** C5c.5 は `metadata_match_labels` に strong ラベルを書くが、H2 でそれをやると production の割り当てに影響し、PF0D の「backflow 無し」に反する。H2 の GT は audit 側 ledger にだけ書く（§6、§12-Q1）。
4. `none` は **明示的な操作のみ**。candidate_count=0 でも自動で none にならず、none の送信には **自前の TMDB 検索を最低 1 回行った記録**を要求する（§4.5）。
5. 946 件の full review は **約 19〜25 時間・約 20 session** の見積り（§11）。実測前の仮定値であり、保証ではない。

---

## 1. Blind 条件

### 1.1 reviewer が知ってよいもの（ALLOW）
| 区分 | 内容 |
|---|---|
| 手元の証拠（raw） | `original_file_name`、`original_rel_path`、`source_media_kind`、`part_no`、`container_tags`（DB 文字列の literal をそのまま）、ファイル数 |
| 候補（表示するもの） | 各候補の `tmdb_id` / `media_type` / `title` / `original_title` / `year` / `overview` / `poster_path` / `original_language`（whitelist 投影。§3.2） |
| 進捗 | 完了数 / 残数 / deferred 数 / その session の経過時間 |
| 自分の過去の判断 | 自分が出した decision と note（resume のため） |
| TMDB の live 情報 | pick_other の検証結果、自分で検索した結果（TMDB の並びであり matcher の順位ではない） |
| 契約・意味 | 4 つの decision の定義（§2） |

### 1.2 reviewer が知ってはいけないもの（DENY）
rules-safe / rules-tags-shadow / rules-1 の **decision（AUTO・REVIEW・UNRESOLVED）・verdict・理由コード**、matcher score、**candidate rank / 並び順の由来**、candidate の **source（どの検索・どの matcher が出したか）**、候補が複数 matcher の和集合であること、Jev の全出力、promotion 結果、coverage / abstention / precision など集計成績、PF0C の閾値到達状況、H1 の中身、他 reviewer の判断、**自分以外の unit に対する自分の decision の分布（none 率など。自己較正を誘うため表示しない）**。

加えて **identity を辿れる値**を出さない: `work_id`、`run_id`、`evaluation_unit_key`、絶対パス、`file_path`、`first_seen_at`、`enrollment_context`。理由: `work_id` やファイル名があると、reviewer が本体アプリの library を開いて production の match_status・poster を見られる（= 機械出力の漏洩）。UI の unit は opaque な `review_unit_id` だけで識別する。

### 1.3 既存 C5c.5 の入口を流用しない理由（コードで確認した事実）
| 既存の挙動 | blind 上の問題 |
|---|---|
| `list_tasks` が `candidate_count` を一覧に出す | 0 件が見えて none への誘導になる。一覧の件数は candidate pool の形（機械の挙動）を示す |
| `ReviewTaskDetail.evidence` が `input_snapshot_json` を **丸ごと** 出す | 中身が whitelist されておらず、matcher 由来フィールド（`effective_provenance` 等）が混ざりうる |
| `ReviewTaskSummary/Detail` が `work_id` / `run_id` / `sample_id` / `cohort` を出す | 本体アプリで引ける。cohort=live は H2 の素性 |
| 候補の並びが `ORDER BY media_type, tmdb_id` | 機械の rank は持ち込まないが、固定ソートは tmdb_id の古さ＝有名作品順に偏る。位置バイアスは固定ランダム順の方が良い |
| `ReviewCandidate.tmdb_snapshot` が `Map<String, Value>` の素通し | 未知キーが通る。whitelist 投影が必要 |
| Tauri アプリ内の画面 | 同一プロセスに production DB・他画面（matcher 結果・Jev）があり、遮断を証明できない |
| 結果を production の `metadata_match_labels` に書く | §0-3 |

**維持するもの**（そのまま再利用する意味・設計）: ready/deferred/resolved の状態、`TargetIsCandidate`、`CandidateNotInRun`（candidate は artifact から読み、client の tmdb_id を信じない）、pick_other の `TmdbTargetVerifier` 3 段手順（lock → 通信 → lock で再確認）、strict 読み（壊れた記録を既定値で救わない）、壊れたら止める fail-closed。

---

## 2. Human GT semantics（C5c.5 から不変）

| decision | 意味 | 送ってよい payload | 禁止 |
|---|---|---|---|
| `confirm` | **表示された candidate が正解** | `candidate_token`（その unit の artifact 内の候補。opaque） | client から tmdb_id を受けて信じること |
| `pick_other` | 正しい TMDB work は存在するが、表示 candidate にない | `tmdb_id` + `media_type`（movie/tv）。**live 検証必須**（§8） | 表示 candidate の ID（`TargetIsCandidate`）、未検証 |
| `none` | 対応する TMDB work が存在しない | 検索ログ（§4.5） | **candidate_count=0 や検索 0 件からの推論**。暗黙の none 禁止 |
| `defer` | genuinely undecidable | `defer_reason`（列挙。§7） | none への変換、reason なし |

ラベルへの写像は既存と同一（parity test で固定。§10-P）:
`confirm → (label=tmdb, method=review_confirm)` / `pick_other → (tmdb, review_pick_other, tmdb_verified=true)` / `none → (none, review_none, identity=NULL)` / `defer → ラベルなし（state=deferred）`。strength は常に strong（defer は non-label）。

PF0C 整合: AUTO かつ GT 未解決（defer 残）は INCONCLUSIVE。reviewer は AUTO を知らないため、**defer を全 unit について解消に向けて扱う**（§7）。

---

## 3. 設計 1: reviewer input artifact

### 3.1 構成（3 ファイル + 1 封印）
```
CineMantis-audit\pr4\pf0g\<build_id>\
  reviewer_artifact\            ← reviewer プロセスが読める唯一の入力
    manifest.json               unit 数・順序 commitment・schema_version・artifact_sha256
    units\<review_unit_id>.json 1 unit = 1 ファイル（whitelist 投影済み）
  sealed_unblind\               ← reviewer プロセスからは読めない（別ディレクトリ・別 ACL）
    unblind_map.json            review_unit_id → evaluation_unit_key / enrollment record seq
    build_attestation.json      machine freeze commitment・PF0E ledger head・packager sha
  review_ledger\                ← reviewer が append する（§6）
```
packager は **一方向**。入力は (a) 凍結済み machine output の「候補集合（unit ごとの TMDB id/media_type の集合のみ）」、(b) PF0E の固定 payload（`matcher_input_payload` の raw フィールドのみ）、(c) TMDB 候補 snapshot。出力に decision・score・rank・source を **構造的に載せられない型**（Rust の別 struct。machine 型からの `From` を持たない・フィールド単位で射影）で組む。

### 3.2 unit ファイル schema（`pr4-blind-review-unit-1`）
```jsonc
{
  "schema_version": "pr4-blind-review-unit-1",
  "review_unit_id": "ru_<26 chars base32 random>",   // work_id/ledger キーと無相関
  "position": 417,                                     // 提示順（seeded permutation。work id 順ではない）
  "evidence": {
    "files": [ { "part_no": 1, "original_file_name": "...", "original_rel_path": "...",
                 "container_tags_json": "<literal string>", "source_media_kind": "..." } ],
    "display_title": "..."                             // §12-Q3 で扱いを確認
  },
  "candidates": [                                      // 固定ランダム順。rank と無関係
    { "candidate_token": "ct_<20 chars base32>",       // HMAC-SHA256(build_secret, domain + review_unit_id + media_type + tmdb_id)。§17 R4
      "title": "...", "original_title": "...", "year": "1999",
      "overview": "...", "poster_path": "/x.jpg", "original_language": "en" }
  ],
  "unit_sha256": "<canonical sha of this object without this field>"
}
```
- `candidates` は **frozen 候補集合の全件**（truncate 禁止。`TargetIsCandidate` 判定の母集合と一致させる）。matcher 間の重複は (media_type, tmdb_id) で dedupe。`candidate_token` は opaque で、rank・identity・順序を含意しない（identity は seal 後の audit-only unblind map にだけ置く。§17 R4）。
- 順序: `shuffle(seed = SHA256(review_unit_id ‖ build_secret))`。**rank・score・source は shuffle の入力にも使わない**（テストで固定）。
- `deny_unknown_fields`。未知キー・型違い・重複 candidate・`candidate_token` 重複・tmdb_id≤0・media_type が movie/tv 以外は packager が拒否（artifact を作らない）、reviewer 側も読込時に拒否。
- 禁止キー/値は §10-L の canary で検証。
- canonical JSON は PF0E の `pr4-canonical-json-1` を再利用。

### 3.3 manifest
`{schema_version, build_id, unit_count, order_commitment_sha256, artifact_sha256, machine_freeze_commitment_sha256(値のみ。中身なし), pf0e_ledger_head_sha256, pf0e_record_count, packager_sha256, built_at_utc}`。**`unit_count` は PF0E の close record の 946 と一致必須**。`machine_freeze_commitment` は「machine freeze が review 開始前に存在した」ことの時系列証明に使う値であり、reviewer には内容ごと見せない（hash 値のみ audit 用に manifest に持つが UI には出さない）。

---

## 4. 設計 2: reviewer UI

### 4.1 実装形態（推奨）
**専用プロセス（loopback HTTP サーバ + 静的 HTML）**。`127.0.0.1` 固定、起動ごとのランダム token、Origin 検査。production DB のハンドル・Tauri command 群・他画面を持たない。TMDB API key は環境変数からバックエンドだけが読み、ブラウザには送らない（値も長さも出さない）。代替案（Tauri の別ウィンドウ）は同一アプリ内に production 資産が残るため非推奨（§12-Q2）。

### 4.2 画面
1. **開始**: reviewer_id、attestation（§9.1）、artifact_sha256 の表示。
2. **レビュー画面（1 unit）**: 左に raw 証拠（ファイル名・相対パス・タグ）、右に候補カード（固定順）。下部に 4 つのボタン: 確定(confirm: 候補選択) / 候補外を指定(pick_other) / 該当なし(none) / 保留(defer)。候補 0 件のときは「表示された候補がありません。**これは「該当なし」ではありません**。TMDB で検索してください」を常時表示。
3. **TMDB 検索パネル**: 自由検索（movie/tv）。結果は TMDB の並びのまま。候補に含まれる結果には「表示済み候補です」バッジ（`TargetIsCandidate` と整合。新情報ではない）。
4. **pick_other 確認**: ID 入力または検索結果から選択 → live 検証結果（タイトル・年・あらすじ・poster）を表示 → 「この作品で確定」の 2 段階。
5. **none 確認**: 検索ログ件数を表示、確認ダイアログ（「TMDB に該当が無いことを確認しました」）。
6. **進捗バー**: 完了/残/deferred のみ。**decision の内訳は出さない。**
7. **session 終了**: 未送信 draft を保存して lease 解放。

### 4.3 提示順
キューは `position` 昇順（seeded permutation）。deferred は **本流が尽きるまで出ない**（§7）。

### 4.4 UI/JSON に絶対に載せない（再掲 + 実装規約）
API 応答は **型付き DTO（`Serialize` のみ・`#[serde(deny_unknown_fields)]` の入力 DTO）** で返し、`serde_json::Value` の素通しを禁止（ソース走査テストで固定）。エラーメッセージにも内部 ID を出さない。

### 4.5 none の条件
`none` 送信には、同一 lease 内で **TMDB 検索を ≥1 回**実行した記録（`search_log`: クエリ文字列・media_type・結果件数・実行時刻）が必須。UI は検索せずに none ボタンを有効化しない。ただしこれは「検索を試みた」証跡であり、none が正しいことの証明ではない（人の判断）。API 側でも検証（UI 迂回の送信を拒否）。

---

## 5. 設計 3: review session lifecycle

### 5.1 unit の状態機械
```
unopened ─lease→ leased ─submit(confirm|pick_other|none)→ resolved   (terminal*)
                    │  ├─defer(reason)→ deferred ─resume→ leased
                    │  ├─lease expiry / release→ unopened(draft 保持) または deferred に戻る前状態
                    └─(2 巡目の再 defer)→ defer_final                  (terminal)
* resolved は correction のみ可（§5.3）。
```
### 5.2 session
`session_start {session_id, reviewer_id, artifact_sha256, tool_sha256, attestation, started_at}` → N units → `session_end {reason: normal|timeout|crash_recovered}`。1 session の上限 **50 unit または 90 分**（先に来た方）で強制休憩（疲労管理。速度で判断を自動化はしない）。ツール・artifact の SHA が前 session と違えば **開始拒否**（途中で build を変えない）。

### 5.3 correction
resolved 後の訂正は `correction {unit, prev_event_sha, reason(列挙)}` + 新 decision。**履歴は全て残し、最新の有効 decision を使う**。訂正は review seal（§9）まで。訂正数・訂正率は completion audit に出る。訂正を禁止するか許すかは §12-Q5。

---

## 6. 設計 4・5: 重複 / 並行 review の防止、resume

### 6.1 保存先: audit 側 append-only ledger（PF0E と同形式）
`review_ledger\events.jsonl`。1 行 1 event。
`event_commitment = SHA256(canonical(event without commitment) + "\n" + prev_commitment)`（先頭 prev は 64 個の 0）。append は `flush + fsync`。**production DB は一度も開かない。**

### 6.2 lease（並行防止）
- ledger ディレクトリに `review.lock`（`create_new` による排他作成 + PID + 起動 nonce）。**プロセスは常に 1 つ**。2 つ目は起動拒否。
- unit 単位の lease: `lease {unit, lease_id, reviewer_id, session_id, expires_at(=now+15分), heartbeat}`。同一 unit に有効 lease があれば他 session は取得不可。
- 提出は `prev_event_sha`（unit 末尾 event の commitment）を CAS として要求。不一致は **409、副作用ゼロ**。
- lease 失効後の提出は、新 lease が無ければ拒否（draft として残すのみ）。

### 6.3 二重提出の扱い
各提出は client 生成の `submission_id`（UUID）を持つ。
- 同じ `submission_id` + 同内容 → **冪等**（同じ結果を返し、event を増やさない）。
- 同じ unit に別 `submission_id` で resolved 後に提出 → 拒否（`UnitAlreadyResolved`）、内容が同じでも event を足さない。correction だけが別経路。
- 同じ `submission_id` で内容が違う → 拒否 + `integrity_anomaly` event（ledger に記録して STOP）。

### 6.4 resume
状態は **ledger からだけ再構築**（メモリや UI の state を信じない）。起動時に chain 全体を検証。unit ごとの最終状態、有効 lease、draft、deferred キューを復元。
- 末尾の torn line（書き込み途中で crash）: 検出して隔離し、operator 承認なしに自動修復しない（STOP）。
- 以前の lease は期限切れとして `lease_expired` event を追記してから再取得。
- resume 後に同じ unit を重ねて出しても、§6.3 で冪等/拒否。
- 前回公表した `(event_count, head)` を session 終了時に別 checkpoint chain へ書き（PF0E §Amendment-3 と同方式）、**ledger 末尾削除を次回検出**する。

---

## 7. 設計 6: defer handling

- `defer_reason` は列挙: `insufficient_evidence`（ファイル名・タグでは作品が絞れない）/ `multiple_equally_valid_targets` / `candidate_ambiguity_tmdb`（TMDB 側の重複・分割）/ `need_external_lookup`（本 session で調べきれない）/ `other_with_note`（note 必須）。**機械由来の理由は存在しない。**
- 保存は **state=deferred + event**。ラベルではない。none に変換しない。
- 本流（unopened）が尽きたら **第 2 巡**として deferred を再提示する。同じ reviewer、別 session。第 2 巡で resolved にできなければ `defer_final`（理由必須）。第 3 巡はない。
- PF0C 上の帰結: `defer_final` は GT 未解決。AUTO と重なれば INCONCLUSIVE になる。reviewer はそれを知らない（blind を保つ）。**だから defer を「楽をするための逃げ道」にしない**: 第 1 巡の defer 率を completion audit に出す（閾値は置かない。報告のみ）。
- 運用上の閾値を後から置くと policy 変更に当たるので、**defer 率に基づく自動処理はしない**。

---

## 8. 設計 7: pick_other の TMDB existence verification

既存の 3 段を維持・強化する:
1. **lock 内**: unit が leased・本 reviewer の lease、`(tmdb_id, media_type)` が artifact 候補集合に **無い**（あれば `TargetIsCandidate`）、media_type ∈ {movie, tv}、`tmdb_id` は 1..=2^53 の整数。
2. **lock なし**: `GET /{media_type}/{id}`（TMDB detail）。応答の `id` と `media_type` が要求と一致することを確認（別作品へのリダイレクト拒否＝既存 `preview_target` と同じ）。HTTP 404 / 429 / 5xx / timeout を区別して記録。
3. **lock 内で前提を再検証** → ledger に追記。
- 記録（provenance）: `tmdb_verification {requested_id, requested_media_type, http_status, response_id, response_media_type, response_body_sha256, title, year, fetched_at, verifier_impl, api_version}`。**APIキーは記録しない。**
- 通信不能 / 429 / 5xx → **提出不可**。`pending_verification` draft として保持し、defer にも none にも変換しない。再試行は同 lease 内で可。
- 「存在する」の定義: TMDB が 200 で同一 id・同一 media_type を返したこと。adult / 削除済み / 統合済み（id が変わる）は `TargetNotVerified`。
- 検証の再現性: 応答本文そのものを audit 側に `tmdb_cache\<sha>.json` として保存（後から存在確認を再監査できる）。

---

## 9. 設計 8・9: provenance と reviewer completion audit

### 9.1 provenance（decision event の必須フィールド）
`{event_type, seq, review_unit_id, submission_id, reviewer_id, session_id, lease_id, artifact_sha256, unit_sha256, tool_sha256, ui_protocol_version, review_protocol_version("gt-review-1" と互換の意味), decision, candidate_token|null（confirm のみ。event に tmdb_id / media_type / index は記録しない）, tmdb_id|null（pick_other のみ）, media_type|null（pick_other のみ）, defer_reason|null, note|null, search_log[], tmdb_verification|null, shown_at, submitted_at, active_ms, prev_event_sha, event_commitment}`
- `tmdb_id/media_type` は confirm では **artifact から解決して記録**（client 値は使わない）。none では必ず null。
- `session_start` に attestation: 「このセッションの前に、この review set に対する機械出力・本体アプリ上の match 結果・Jev を見ていない。review 中も本体アプリの library を開かない」。自己申告であり、残余リスクとして §12-Q6 に明示。

### 9.2 Completion audit（review seal の前提。すべて満たさなければ seal しない）
| # | 検査 | 失敗時 |
|---|---|---|
| A1 | ledger の hash chain 全体が検証できる。公表済み checkpoint と一致 | STOP |
| A2 | manifest の `unit_count`（=946）と、terminal 状態の unit の数が一致。terminal = resolved の 3 種 or `defer_final` | seal 拒否 |
| A3 | 各 unit に有効な decision がちょうど 1 つ（correction 後の最新）。未解決の leased/draft/unopened が 0 | seal 拒否 |
| A4 | `confirm` の candidate_token が当該 unit の artifact に実在。`none` は tmdb_id=null。`pick_other` は全件 `tmdb_verification` 付き・候補外 | seal 拒否 |
| A5 | `none` は全件 search_log ≥1 | seal 拒否 |
| A6 | unit_sha256 / artifact_sha256 / tool_sha256 が全 session で一定 | seal 拒否 |
| A7 | 提出の `submission_id` に重複なし。integrity_anomaly event が 0 | STOP |
| A8 | **leak scan**: reviewer プロセスが返した全 API 応答の型ごとのスキーマハッシュが allowlist と一致（ランタイムで応答 body の type tag を記録） | STOP |
| A9 | 時系列: machine_freeze_commitment の存在時刻 < 最初の session_start | seal 拒否 |
| A10 | 報告のみ（閾値なし）: decision 内訳件数・defer 率・correction 数・session 数・active 時間・pick_other 件数 | — |

seal 後に **初めて** `sealed_unblind\unblind_map.json` を開いて evaluation_unit_key に join できる。seal event に `{ledger_head, event_count, unit_count, decision_counts, audit_report_sha256}` を書く。seal は不可逆。**seal 前に evaluator を走らせる経路を作らない**（同一バイナリに入れない）。

---

## 10. テスト計画（synthetic または旧 C5c.5 fixture のみ）

H2・H1・実 machine output・production DB は使わない。fixture は synthetic ジェネレータ（既存 `db::test_support` の流儀）+ `gt_review.rs` の既存テスト fixture。TMDB は `TmdbTargetVerifier` の fake。ネットワークなし。

### L: 漏洩（最重要）
- **L-1 canary**: synthetic machine output の **全フィールド**（decision、score、rank、source、理由コード、Jev）に一意の canary 文字列/数値（例 `CANARY_SCORE_0.87314`、rank=9137）を埋め、packager → artifact → サーバの全 API 応答 → HTML/JS → ログ → ledger の **全バイト列**を検索して 0 件。
- **L-2 禁止キー**: 全 JSON を再帰走査し、`decision|verdict|AUTO|REVIEW|UNRESOLVED|score|rank|rules|jev|matcher|source|promotion|precision|coverage|abstention` の key・value を拒否（大小・区切り違いも）。
- **L-3 構造**: reviewer モジュールが machine 型・production DB 型・`rusqlite`・`metadata_match_*`・PF0E ledger のキー型を参照しないことのソース走査（PF0E と同方式）。
- **L-4 順序独立**: candidate の `rank`/`score`/`source` を変えても artifact（バイト単位）が不変。shuffle の入力に rank が無いこと。
- **L-5 件数の非露出**: 一覧/進捗 API に `candidate_count` が無い。0 件 unit でも none が自動選択されない。
- **L-6 識別子**: 応答に `work_id`/`run_id`/`evaluation_unit_key`/絶対パス/`first_seen_at` が無い。
- **L-7 エラー経路**: 異常系（壊れた unit・409・検証失敗）のエラー文面にも canary と内部 ID が出ない。
- **L-8 whitelist**: candidate の snapshot に未知キー（`rules_rank` 等）を足した入力を packager が拒否。

### S: 意味（semantics）
- **S-1 confirm**: 候補を選び、(tmdb_id, media_type) が artifact から解決される。client が別 ID を送っても無視/拒否。
- **S-2 pick_other**: 候補外・検証 OK で resolved。`tmdb_verification` が付く。候補内 ID は `TargetIsCandidate`。fake が別 id を返す→拒否。404/429/5xx/timeout→提出不可・draft 保持。
- **S-3 none**: search_log ≥1 で resolved、identity=null。search_log なしは拒否。**候補 0 件の unit でも自動 none にならない**（明示操作が要る）。
- **S-4 defer**: reason 必須。deferred は本流が尽きるまで出ない。第 2 巡で resolved/defer_final。none に変換されない。
- **S-P parity**: 同じ synthetic fixture で、新 core の decision→(label, tmdb_id, media_type, method, tmdb_verified) が **既存 `gt_review::resolve_*`（in-memory DB）の出力と一致**する差分テスト。

### R: resume / 並行 / 重複
- **R-1 resume**: 各 event の直後に強制終了 → 再起動で状態が ledger から同一に復元される（全 event 境界でループ）。
- **R-2 torn line**: 末尾が不完全な行 → 検出して STOP（自動修復しない）。
- **R-3 重複提出**: 同 submission_id・同内容→冪等、違う内容→異常 STOP、resolved 後の別 submission→拒否。
- **R-4 並行**: 2 スレッドが同 unit を lease/submit → 片方のみ成功。2 つ目のプロセス起動は `review.lock` で拒否。
- **R-5 lease 失効**: 失効後の提出が拒否され draft が残る。
- **R-6 CAS**: stale な `prev_event_sha` → 409 副作用ゼロ。
- **R-7 改ざん**: event の改変・中間削除・並べ替え・末尾削除（checkpoint 不一致）を検出。

### M: malformed
- **M-1 target**: tmdb_id = 0 / 負 / 小数 / 文字列 / 2^63 超 / null、media_type = "" / "person" / "TV" / 余計な空白 / 配列。
- **M-2 参照**: 存在しない review_unit_id、他 unit の candidate_token、`../` を含む unit id（パス traversal）、長大文字列、制御文字。
- **M-3 artifact**: schema_version 違い、unknown field、重複 candidate、unit_sha256 不一致、manifest の unit_count 不一致、欠けた unit ファイル→ **起動拒否**（既定値で救わない）。
- **M-4 JSON**: 壊れた JSON、BOM、巨大 body、`submission_id` 欠落。

### A: completion audit
- **A-T** §9.2 の A1〜A9 を、それぞれ 1 件ずつ違反させた fixture で seal が拒否されること。全て満たす fixture でだけ seal が通ること。seal 前に `unblind_map` を読む API が存在しないこと。
- **E-1 rehearsal**: synthetic 946 unit + 擬似 reviewer（決定論的スクリプト）で全工程（lease→提出→defer→2 巡目→seal）を通す。所要を測るのは **ツール側オーバーヘッドのみ**。人の速度は測れない（§11）。

---

## 11. 設計 10: 946 件 full review の feasibility

**前提と限界を先に述べる**: 以下は実データを見ていない仮定による見積りで、実際の難易度分布は未知。

| unit 種別（仮定） | 割合 | 1 件あたり |
|---|---|---|
| 候補に正解があり明白（confirm） | 70% | 45 秒 |
| 検索や比較が要る（confirm 難/pick_other） | 20% | 90 秒 |
| 該当なし確認・曖昧（none/defer） | 10% | 240 秒 |

平均 ≈ 73.5 秒/件 → 946 × 73.5 ≈ **19.3 時間**。休憩・session 切替・TMDB 待ちで ×1.3 → **約 25 時間**。2 巡目の defer（5〜8% を想定）を足して +1〜2 時間。
- session 50 件 ≈ 1.0〜1.3 時間 → **約 19〜20 session**。1 日 2 session なら **約 10 日**、1 日 1.5 時間なら約 3 週間。
- 速い側（45 秒平均）で約 12 時間、遅い側（120 秒平均）で約 32 時間。**どちらでも「1 人が数週間で完遂可能」な規模。**
- ボトルネックは UI 速度ではなく **pick_other / none の判断**。search パネルと poster/overview 表示が時間短縮の主因。
- **疲労対策**: 50 件/90 分上限、休憩強制、同一の簡単な判断が続くことによる確認バイアスに対し、提示順は seeded random（難易度で並べない）。
- **二重 review は 2 倍（約 40〜50 時間）**: 全件は非推奨。推奨は **独立 blind 再 review の 10% 抽出（約 95 件・事前に seed を commit）**で agreement を測る（報告のみ・decision は上書きしない）。§12-Q4。
- 待つ時間: H2 の 946 件到達自体が数か月オーダーの可能性（PF0D）。reviewer の準備は 946 到達前に synthetic で完成させておく。

---

## 12. 未解決の問い（レビュー依頼）

| # | 問い | 推奨 |
|---|---|---|
| Q1 | H2 の GT を **audit 側 ledger にだけ**書き、production の `metadata_match_labels` に書かない方針でよいか。この場合 evaluator（PR4-2 の `trusted_gt_source` allowlist）に **新しい GT source の amendment**（例 `pr4-h2-blind-gt-1`）が要る。これは PF0G0 の範囲外で別 gate | 推奨: ledger のみ + 別 amendment |
| Q2 | UI 形態: 専用 loopback サーバ（推奨）か、Tauri 別ウィンドウか | 専用プロセス |
| Q3 | `display_title`（parser が作る `derived_title`/`title_guess`）を見せてよいか。matcher の入力側だが parser の出力で、anchoring の余地がある | 初期は **見せない**（raw ファイル名のみ）。時間がかかりすぎる場合のみ amendment |
| Q4 | 独立 second review の範囲: なし / 10% 抽出（推奨）/ 全件。reviewer は誰か（あなた 1 名か、第三者か） | 10% 抽出、別 session |
| Q5 | review seal 前の correction を許すか。許せば精度が上がるが「結果を見て直す」余地は無い（blind なので無い）。禁止すれば誤入力が確定する | 許可・全履歴保存・訂正率を報告 |
| Q6 | reviewer が本体アプリの library で H2 作品の production match を見てしまうリスク（file 名から検索可能）。運用規約 + attestation 以上の遮断は技術的に不可能。許容するか | 許容 + attestation（残余リスクとして明記） |
| Q7 | pick_other の TMDB 検証に使う API キーの供給方法（env var 推奨。production DB から読むのは避ける） | env var |
| Q8 | `defer_final` を許す上限は持たない（PF0C の閾値に影響するため）。ただし defer_final 率が高いと INCONCLUSIVE のリスクがある点を、あなたが事前に承知するか | 承知の上、上限なし |
| Q9 | 検索パネルで TMDB 検索を許すことで、candidate 以外の情報源を与える。candidate set 自体が機械由来であり confirm の anchoring は残る（C5c.5 と同じ）。検索を **必須の none 証跡**にすることで緩和する方針でよいか | 緩和策として採用 |
| Q10 | machine freeze → artifact build → review の順序を、どの commitment で時系列証明するか（freeze commitment の存在時刻をどこに固定するか） | PF0E の checkpoint chain 方式を踏襲 |

---

## 13. Implementation plan（承認後・この gate では実装しない）

| gate | 内容 | 完了条件 |
|---|---|---|
| PF0G1 | `pr4_blind_gt` core（型・canonical JSON・ledger・lease・状態機械・verifier trait・completion audit）。**UI なし。** synthetic のみ | §10 の S/R/M/A テスト green、L-3 のソース走査 green |
| PF0G2 | packager（一方向・whitelist 投影・canary テスト）+ 型付き DTO の API サーバ | L-1〜L-8 green |
| PF0G3 | 静的 UI（検索パネル・pick_other 2 段・none 条件・進捗） | UI 経由で S 系の E2E（built-in browser で synthetic）。a11y と日本語文言の確認 |
| PF0G4 | rehearsal: synthetic 946 + 擬似 reviewer、旧 C5c.5 fixture での parity | E-1・S-P green。実装 freeze（SHA 固定・commit）|
| PF0G5 | （H2 到達 + machine output freeze 後）実 artifact build → review → completion audit → seal | A1〜A9 全 pass |

新規ファイルの予定: `services/pr4_blind_gt/{mod,ledger,lease,artifact,audit,verifier}.rs`、packager（test-gated binary または example）、reviewer サーバ（別 bin）、静的 HTML。**既存の `gt_review.rs` / `gt_operator.rs` / `commands/review.rs` は変更しない**（意味は parity test で保護）。main checkout は触らない。

## 14. 触れていないこと（確認）
H2 実データ（`pf0e` 配下）・H1・machine output・Jev・production DB は読んでいない。main checkout は未変更。commit / push はしていない。

---

## 15. Amendment r2（監査裁定）と PF0G1 の実装範囲

裁定: Q1 audit ledger のみ（production `metadata_match_labels` に書かない）/ Q2 専用 loopback process / Q3 `display_title` は見せない / Q4 10% QC repeat（seed は review 開始前に ledger へ freeze。**別人でなければ "independent second review" と呼ばない**）/ Q5 seal 前 correction 可（全履歴・理由・訂正率を保存）/ Q6 review 中に本体 CineMantis process が動いていれば STOP、残余の認知的漏洩は attestation / Q7 TMDB key は env var / Q8 `defer_final` 上限なし / Q9 human TMDB search panel 採用・none には検索証跡 ≥1 / Q10 machine freeze の時系列証明は独立 audit anchor chain。
reviewer candidate set は PF0F で凍結された `rules-tags-shadow-2` の review projection のみ。candidate の再 fetch 禁止、overview 本文も PF0F artifact 由来。

PF0G1 実装（`services/pr4_blind_gt.rs` + `pr4_blind_gt_tests.rs`、synthetic のみ、23 テスト green）:
ledger（hash chain・fsync・torn tail/改変/削除/並べ替え検出）/ checkpoint chain（末尾削除検出）/ lease・CAS・session 上限（50 unit・90 分）/ 冪等な二重提出と異常 STOP / resume（全コマンド境界で再起動しても同一 head）/ correction 履歴 / defer 2 巡と defer_final / QC plan（seed freeze・ceil 10%）/ opaque `review_unit_id`・提示順 / completion audit と seal。
実装上の判断: QC の対象には defer_final の unit も含める（機械の結果を知らないため選別できない）。A8（leak scan）は PF0G2 のサーバが出す report の hash を `seal()` の必須引数にした。
PF0G2（packager・UI・TMDB 検証の実通信）は PF0F1 schema freeze 後。

## 16. PF0G1 freeze 前の最終確認 4 点（監査 HOLD への対応）

1. **candidate は opaque token。**（§17 R4 が現行の正本。ここにあった旧定義 `H(unit_salt | media_type | tmdb_id)` と、「`unit_salt` から token を計算して pick_other の重複を照合する」旧方式は **obsolete**。）reviewer package の candidate は `candidate_token`（`ct_` + 20 文字。定義は §17 R4 の HMAC）と表示用 projection（title / year / overview / poster 等）だけ。`tmdb_id` / `media_type` / index / rank を持たない。confirm の ledger event は `candidate_token` **だけ**を記録し、identity は書かない（identity を持つ confirm event は replay で STOP）。token → identity の対応表は seal 後の audit-only unblind map にだけ置く。pick_other が表示候補と重なるかは、unit の `overlap_digests`（§17 R4）で照合する。
2. **reviewer package の hard check。** package に置けるのは `manifest.json` と `units/<id>.json` だけ。それ以外のファイル・ディレクトリ、および中身に `build_secret` / `unblind` / `work_id` / `"work:` / `evaluation_unit_key` / `candidate_mapping` / `identity_map` / `token_map` / `"tmdb_id"` があれば `Artifact::load` が STOP する（PF0G2 のサーバも同じ関数を起動時に呼ぶ）。`review_unit_id` を secret から導出すること自体は問題ない（secret は package に入らない）。
3. **init と seal の pin。** 次の 8 個を init event に記録し、seal では引数と init の **全項目の完全一致**を要求する（不一致は `PinMismatch` で pin 名を返す。replay でも検査するので、後から差し替えた seal は open できない）: `protocol_sha256` / `ui_package_sha256` / `shuffle_seed_commitment` / `attestation_policy_sha256` / `unblind_map_sha256` / `build_secret_commitment` / `a8_leak_scan_report_sha256` / `machine_freeze_anchor_commitment`。`a8_leak_scan_report_sha256` は **review 開始前に取った package / UI の leak scan report**として pin する（review 中の実行時 scan は PF0G2 の範囲）。`Store::seal` は `(AuditReport, review_seal_sha256)` を返し、`review_seal_sha256` は seal event の `event_commitment` である。これを外部 audit anchor に固定する（外部への書き込みは operator の gate）。
4. **post-seal errata。** sealed GT は immutable。seal 後に明白な GT の誤りが判明した場合、GT は書き換えない（seal 後の correction / lease / session は全て `Sealed` で拒否）。`Store::record_post_seal_errata` は `errata.jsonl`（`review_seal_sha256` に結び付いた別の hash chain）に記録するだけで、`events.jsonl` には書かない。`promotion_impacting = true` の errata が 1 件でもあれば `evaluation_validity()` は `Invalidated` になり、**新しい evaluation cycle が必要**。結果を見て GT を直す余地は無い。

HEAD の記述: PF0G の worktree は 0aca746 を base にしており正常。main checkout（C:\Users\tyama\CineMantis）は本日確認時点で `feat/awards-festivals` の 69d150d で、PF0E-3 採用後の進行と一致する。この session は main checkout を一切変更していない（前報の「0aca746」は PF0G worktree の base を指すべきだった）。

## 17. 監査差し戻し反映（D7 / R1〜R4 / U3）— 本節が §16 の 1 に優先する

- **D7（build_secret の隔離）:** reviewer package = `review_unit_id` / `candidate_token` / frozen display projection / protocol・UI artifacts のみ。`build_secret` と `unblind_map` は audit-only の sealed side にあり、reviewer 側から完全に隔離する。review 開始前に reviewer 側へ渡すのは `build_secret_commitment` と `unblind_map_sha256`（pin）だけ。実体を受け取るのは PF0I adapter で、review seal 後。
- **R1（process-level blind）:** package に map / secret が無い（`check_reviewer_package`）/ CineMantis 本体が動いていれば reviewer 起動拒否（`MainAppRunning`）/ leak scan PASS（`a8_leak_scan_report_sha256` を pin）/ reviewer attestation。
- **R2（unit key）:** `work:<base10 integer>`。先頭ゼロ・符号・空白・全角数字は禁止（`validate_unit_key`）。reviewer には見せない。
- **R3:** PF0I が exact-match するのは `authoritative_membership_commitment` / PF0H final manifest SHA / review seal SHA のみ。
- **R4（token）:** `c1` / `c2` は廃止。`candidate_token = HMAC-SHA256(build_secret, domain + review_unit_id + media_type + tmdb_id)`、`domain = pr4-pf0g-candidate-token-1`（区切りは `
`、`ct_` + 20 文字）。token は packager だけが計算できる。ledger には `candidate_token` のみを保存し、TMDB identity は seal 後に unblind map から復元する。
  - 帰結: reviewer は build_secret を持たないので token を再計算できない。pick_other の「表示候補との重複」判定のため、unit は `unit_salt` と `overlap_digests`（`SHA256(unit_salt|overlap|media_type|tmdb_id)` の昇順一覧）を持つ。token との対応は付かない（件数と順序のみ。総当たり可能性は §16 の 1 と同じ扱い）。
- **pin:** init と seal の双方に `protocol_sha256` / `ui_package_sha256` / `shuffle_seed_commitment` / `attestation_policy_sha256` / `unblind_map_sha256` / `build_secret_commitment` / `a8_leak_scan_report_sha256` を pin して完全一致を要求する。`machine_freeze_anchor_commitment` は既存の genesis 条件のまま維持し、seal でも同じ値を参照する（実装では 8 個目の pin として同じ完全一致検査に含めた）。
- **review seal SHA の正本** = seal event の `event_commitment`（`Store::seal` の戻り値 / `review_seal_sha256()`）。
- **U3:** seal 後の GT は変更禁止。誤りは errata のみ追加。promotion-impacting なら evaluation = INVALIDATED で、新しい evaluation cycle が必要。
