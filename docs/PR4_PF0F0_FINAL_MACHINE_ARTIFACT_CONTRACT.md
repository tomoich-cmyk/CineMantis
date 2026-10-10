# PR4-PF0F0: FINAL Machine-Output Artifact Contract（r2）

基準 commit: `0aca746e2d0fee12ecfc660c5fc2e40d619158c6`（branch `feat/pr4-pf0f`）。
状態: **r2 = 監査裁定反映済み。PF0F1（synthetic / mock のみ）を実装済み**: [pr4_pf0f.rs](../apps/desktop/src-tauri/src/services/pr4_pf0f.rs)。H2 実行、TMDB live call は行っていない。PF0C の binding policy（99% 片側 CP 下限 / 最低 AUTO 299 / coverage 50%）は変更しない。

この文書は、H2=946 が close した後に「凍結済み H2 input payload から machine output を **1 回だけ**生成し、後段の FINAL evaluator（`pr4_pf0b::evaluate`）へ渡す」ための artifact 契約を、H2 の実データを見ずに先に凍結するためのもの。機械可読 schema 案は [pr4_pf0f0_machine_artifact_schema.json](pr4_pf0f0_machine_artifact_schema.json)。

調査に使ったのは既存コードの read-only 読解のみ（`prematch_snapshot.rs` / `pr4_pf0e.rs` / `commands/tmdb.rs` / `tmdb_client.rs` / `metadata_matcher.rs` / `rules_v1.rs` / `pr4_pf0b.rs`）。production DB・H2 ledger/payload/candidate/GT・H1・Jev・`CineMantis-audit\pr4\pf0e` は一切読んでいない。

---

## r2 改訂（監査裁定。**以下は本文 §1〜§9 の該当記述に優先する**）

| # | 裁定 | 反映 |
|---|---|---|
| 1 | **membership authority = PF0B `membership_commitment` のみ** | manifest `membership.{commitment, eligibility_rule_version, pf0h_final_artifact_sha256}`。generator は起動時に unit key 集合から PF0B 定義で再計算して一致を必須とする。PF0H の `unit_set_sha256` と PF0F の旧 `payload_set_sha256` は `*_aux`（補助監査値。判定に使わない）。manifest は PF0H 最終成果物の sha256 を pin（無ければ起動しない）。evaluator も replay 時に再計算 |
| 2 | **binding = `rules-tags-shadow-2`**（combined-ranked）。`rules-safe-2`（legacy-ranked）は reference/control。rules-1 は対象外 | manifest `binding_matcher` / `reference_matcher` / `rules_1_in_final=false` を起動時に検査。PF0B `Machine` は shadow から写像。Q1・Q4 は解決 |
| 3 | **reviewer projection** | `reviewer_projection.jsonl`: shadow が判定に使った凍結 combined-ranked 集合を `{tmdb_id, media_type, title, original_title, year, release_date, overview 本文, poster_path}` で保存。rank / score / confidence / reasons / query_source / machine decision は含めない。表示順も rank を漏らさないよう `(media_type, tmdb_id)` 昇順。`shown_set_sha256` を持つ。**PF0G が TMDB を再 fetch して表示内容を作ることは禁止**。replay は projection も再導出して一致を検証 |
| 4 | **raw TMDB response** | 成功 response は **本文を `raw/<sha256>.bin`（content-addressed）で保存** + request record に `raw_body_sha256`。§3.4 の projection-only 方針は撤回（Q3 解決）。replay はこの本文から decode して候補を再導出する。失敗 response は bounded evidence（`http_status`, `body_bytes`, `body_sha256` の3項目のみ・本文なし）。API key / Authorization / secret / 派生値は保存禁止（禁止キー名スキャン + 実 key 値スキャン + `api_key=` 検出。ヒットで run 中止）。raw 本文は TMDB の overview 等を含むため audit 内に閉じる（共有しない） |
| 5 | **technical recovery** | primary 後、**retryable な失敗 unit だけ最大 1 回**。retryable = ネットワーク失敗（3 回 attempt 後）/ HTTP 429 / 5xx。decode_error・input_mismatch・invalid_output・その他 4xx は非 retryable。primary 終了直後に `recovery_set.json`（対象 key・primary の chain head・manifest sha・`set_commitment`）を確定してから実行。対象外 unit・成功済み unit は再実行不可。同一 manifest / code / matcher / input / query contract。GT review 開始前に完了（封印前に終えることで強制）。残存失敗があれば PF0B `run_failure` → **TECHNICAL FAILURE**。Q2 解決 |
| 6 | **generation boundary** | 1 manifest = 1 logical generation session（primary + recovery）。`RUN.lock` を `create_new` で排他取得し、2 回目の起動は拒否。封印後は書き込み拒否。provenance に session の `started_at_utc` / `ended_at_utc` を記録。performance を見ての条件変更・追加 run は禁止（手続き上の禁止。コードは再起動を拒否することで強制） |

### r2 での実装上の訂正・変更点

- **matcher バージョンは既存**: `metadata_matcher::POLICY_VERSION = "rules-safe-2"`、`match_history` に `rules-tags-shadow-2`。§4.5 / Q5 の「バージョン定数は無い」は誤り。manifest は `code_commit` + 対象ソース sha と併せて上記ラベルを記録する。
- **production 側の変更は 1 点だけ**: `commands/tmdb.rs` の `merge_candidate_sets` を `fn` → `pub(crate) fn`（挙動不変。generator が同じ merge を使うため）。
- **`derive()`**: `fetch_candidates` の手順を写した関数で、generator と replay が共有する。差は「失敗した call（year なし再検索を含む）を握りつぶさず `Err` にする」点のみ。`fetch_candidates` との同値性は `planned_specs` と `planned_logical_calls` の一致テスト + 実装の対応で担保しており、**live の `TmdbClient` を通した同値性テストは未実施**（transport が差し替え不能なため。Q11）。
- **UNIT_INPUT**: payload 本文は保存しない。evaluator が凍結 payload 集合を持ち、replay 時に `matcher_input_sha256` を再計算して照合する。
- **TMDB_REQUEST**: `wire_query_sha256_without_key` は廃止（URL 組み立ては実 transport の責務）。`canonical_request_sha256` で query plan と照合する。1 論理 call = 1 record（再試行は `attempts[]`、最大 3）。unit には `attempt`（1 or 2）。
- **MATCHER_OUTPUT.technical_status**: `ok | skipped_no_title | call_failure | decode_error | input_mismatch | invalid_output | internal_error`、`retryable` を併記。
- **CANDIDATE_SET**: candidate ごとの request 帰属は持たず、`query_source`（legacy/embedded/both）と `all` の挿入順（ord）+ raw 本文からの replay で再現性を担保する。
- **evaluation_unit.jsonl** は封印時に最新 attempt から生成。adapter は `verify_and_replay` 成功後にのみ `FinalInput` を作る。GT は別経路で渡し、集合が membership と一致しなければ拒否する。
- 実 HTTP transport（api_key 付与・backoff・URL エンコード）は PF0F1 の範囲外。

### r2 追加裁定（Q11〜Q14 解決・実装済み）

| Q | 裁定 | 実装 |
|---|---|---|
| Q11 | derive parity 必須 | `derive()` と、production の純粋ロジック（`query_plan_for` / `parsed_for_query` / `score_*` / `merge_and_rank` / `merge_candidate_sets` / `planned_logical_calls`）を **fixture response** で比較するテスト（query plan・legacy/combined の all と ranked・source・順序・matcher 判定が一致。6 シナリオ: year 再検索と重複除去 / tv のみ / kind 不明 / embedded 追加と merge / embedded のみ / 0 call、同点の挿入順を含む）。加えて production 関数 6 つのソース sha256 を pin し、**変わったらテストが落ちて parity の再確認を強制**する。**限界（明記）**: `fetch_candidates` 自体は `TmdbClient` が差し替え不能なため直接は走らせていない。比較対象の基準実装（oracle）は `fetch_candidates` の本体をテスト内に別途書き写したもの。live TMDB は使わない |
| Q12 | raw frozen response が authority | projection に `release_date`（response の値そのまま）と、既存ロジックが同じ response から導いた `year`、参考の `release_year_from_date` / `year_consistent_with_release_date` を併記。merge で採用された元の response（legacy 優先、embedded は confidence が厳密に高いときのみ）の日付を使う。不一致だけでは unit を除外しない（日付が空でも unit は残り、空文字を verbatim 保存）。review projection は同じ frozen response 由来 |
| Q13 | primary crash の resume | manifest に `unit_order_sha256`（unit 処理順を事前 commit）と `resume_policy.max_primary_resumes`。`Generator::resume`: 同一 manifest・順序の prefix 完了・未完了 unit の痕跡なし・chain 検証を満たすときだけ、完了済み unit を再実行せず未実行 unit のみ続行（session 開始時刻は元の `run_start`、`resume_primary` イベントと provenance `primary_resumes` を記録）。証明できなければ `CONTINUITY_FAILURE.json` を残して失敗し、以後再開も封印も不可（= FINAL は TECHNICAL FAILURE）。recovery 対象確定後・封印後は再開不可 |
| Q14 | transport policy を pin | manifest `transport_policy`（max_attempts / backoff_ms / retry_statuses / retry_after = ignore or honor_capped + cap / concurrency=1 / timeout / pacing）と `policy_sha256`。generator の再試行・待ち（`Transport::pause`）はこの policy だけで決まり、attempts に `backoff_ms` / `retry_after_secs` を記録。hash 不一致・並列度≠1・不正値は起動不可。封印後の `verify_and_replay` も再検査。結果を見ての変更は manifest 凍結で不可 |

### PF0F1 の監査対象に明示する production 変更

`apps/desktop/src-tauri/src/commands/tmdb.rs` の **1 箇所だけ**: `fn merge_candidate_sets(` → `pub(crate) fn merge_candidate_sets(`（visibility のみ・動作変更なし）。`git diff --stat` は同ファイル 1 行 + `services/mod.rs` の test 専用 module 登録 3 行のみ。`cargo test --lib` 全体 571 passed / 0 failed。

### PF0F1 最終条件（監査裁定・反映済み）

1. **resume の固定（fail-closed）**
   - `resume_policy = {max_primary_resumes: 1, recovery_resume: "forbidden"}` に固定。manifest がこの値でなければ起動しない。
   - primary phase: 最大 1 回の resume。2 回目の primary 中断は `CONTINUITY_FAILURE` → TECHNICAL FAILURE。
   - recovery phase: **resume 禁止**。recovery 中断（対象集合の確定後の中断）は `CONTINUITY_FAILURE` → TECHNICAL FAILURE → FINAL promotion 判定へ進まない。結果を見て recovery をやり直す余地を作らない。
   - `CONTINUITY_FAILURE.json` がある run は封印・検証・PF0B 変換のすべてを拒否する。封印済みの run に再開を試みても artifact には何も書かない（index 外のファイルを増やさない）。Q13b は「resume しない」で解決。
2. **Q11b は PF0F2 へ明示的に defer**（PF0F1 の FAIL 条件にしない。実 HTTP transport は PF0F1 の scope 外）。
   - **PF0F2 — TMDB Transport / Wire-Parity Gate**: live TMDB は使わず、loopback / mock HTTP だけで `CanonReq → 実際の HTTP method / path / query encoding → production TmdbClient と同じ wire semantics` を確認する。production の URL 構築はコピーせず、既存の pure な builder を共有するか最小限 `pub(crate)` 化（visibility のみの変更をもう 1 件許容）。
   - 最低限の確認項目: movie / tv、year あり / なし、`language=ja-JP`、`include_adult=false`、query の percent encoding、API key が artifact に残らない、429 / 5xx / timeout、retry policy、request SHA と実送信内容の一致。

### source SHA pin の位置づけ（tripwire であって保証ではない）

production 関数 6 つの source sha256 の pin は「production 実装が変わったら parity review を強制する **tripwire**」であり、**SHA 一致 = semantic parity ではない**。実質的な保証は fixture parity テスト（`derive_matches_the_production_logic_on_query_plan_candidate_set_and_ordering`）であり、それも `fetch_candidates` の書き写しとの比較であって、完全な wire parity の証明ではない（PF0F2 で補う）。

### 残る未決

| Q | 論点 |
|---|---|
| Q11b | PF0F2 へ defer（上記） |

---

---

## 1. 調査結果（コード上の事実）

### 1.1 payload → TMDB query plan

```
H2 payload (pr4-h2-input-payload-1)
  └ snapshot_from_payload(payload, work_id, evidence_class)      … DB を読まない
       └ PreMatchSnapshot.local_evidence() → LocalEvidence
            └ search_inputs(): [Legacy(title=derived_title, year=None),
                                (+Embedded(title=embedded.title, year=embedded.year) ※条件付き)]
                 └ query_plan_for(input, media_kind) → QueryPlan{query, year, search_movie, search_tv, retry_without_year}
```

- Legacy: `rules_v1::search_plan_v1(title, media_kind)`（凍結実装。共有 parser を変えても動かない）。
- Embedded: `parsed_for_query` → `normalized_title` / `year = input.year or parsed.year_hint` / `search_movie = kind != "tv"` / `search_tv = kind != "movie"` / `retry_without_year = year.is_some()`。こちらは共有 `title_parser` に依存する。
- Embedded を足す条件: 正規化後タイトルが Legacy と違う、または embedded_year があって filename_year と違う。
- `skips_tmdb`: `title` が空白のみ **かつ** `embedded_title` が無い → **TMDB を 0 回呼ぶ**（正規の UNRESOLVED。技術的失敗ではない）。
- payload に入らない（= 結果に影響しない）もの: `country_type`、`work_created_at`、`extension`、`duration_sec`、`embedded_ids`。`evidence_class` は matcher が読まないが、payload 外（enrollment context）から渡される。
- `canon()` は浮動小数を拒否する。→ TMDB 応答の `vote_average`（float）は canonical JSON に入れられない（§3.4）。

### 1.2 `search_candidates` の外部依存

| 依存 | 内容 |
|---|---|
| ネットワーク | `TmdbClient`（`https://api.themoviedb.org/3`、timeout 30s、`MAX_HTTP_ATTEMPTS=3` で接続失敗のみ 400ms×n の backoff 再試行。HTTP 4xx/5xx は**再試行しない**で `Err("TMDb HTTP …")`） |
| 認証 | `api_key` は **URL クエリ文字列に埋め込まれる**（`?api_key=…`）。→ URL をそのまま記録すると secret が漏れる。エラー文字列は種別のみで URL を含まない（現状は安全） |
| リクエスト固定値 | `language=ja-JP`、`include_adult=false`、movie は `year=`、tv は `first_air_date_year=`、**page 指定なし（= 1 ページ目のみ）**、region なし |
| エンコード | 自前 `urlencoding`（スペース→`+`、非 ASCII は UTF-8 percent-encode）。クライアント実装の一部なので再現性に含める |
| 呼び出し順 | `inputs` 順（Legacy→Embedded）、各 input で movie → （年ヒントがあれば year なし movie 再検索）→ tv。**逐次**。 |
| 時刻 | matcher / parser に `now()`・年の現在値依存は grep で見つからなかった（実装時にテストで固定）。sleep は batch 側のみで出力に無関係 |

### 1.3 production 実装の「記録に残らない」穴（FINAL では埋める必要あり）

1. **year なし再検索の失敗は握りつぶされる**（`if let Ok(results2) = …`）。失敗しても `errors` にも `tmdb_calls` 以外にも痕跡が残らない。
2. movie/tv の片方だけ失敗しても、もう片方が成功すれば `Ok` を返す（`succeeded > 0`）。部分失敗が「成功」として扱われる。
3. year なし再検索の重複排除は `tmdb_id` のみで `media_type` を見ない（movie バケット内だけなので現状は実害なし。ただし挙動として記録する）。
4. 候補の順序は「各 query の TMDB 返却順 → `merge_and_rank` の **安定ソート**（confidence 降順、同点は挿入順）→ confidence ≥ 40 → 上位 10」で決まる。**同点の順序は TMDB の返却順に依存**し、top 同点は `TIED_TOP` で AUTO にならないが、`top` 自体は挿入順で先のもの。
5. `skips_tmdb` の 0 call は `Ok(空)` で返り、呼び出し側が UNRESOLVED にする。

→ 契約では **すべての論理 call に status を持たせ、部分失敗を unit の `technical_status` に反映**する（§6）。

### 1.4 rules-safe / rules-tags-shadow

- 入力: `candidates`（rules-safe は **`legacy_ranked`**、shadow は **`combined_ranked`**）、`ParsedTitle`（Legacy の parse 結果）、`EmbeddedEvidence{embedded_year, filename_year(=filename_year or title_guess_year), audio_languages, cut_editions, episode_id_marks_movie}`。
- rules-safe = `rules_safe_with_embedded`（年ヒントは `parsed.year_hint` のみ）。shadow = `rules_tags_shadow_best_candidate`（年ヒント `parsed.year_hint or embedded_year`）。
- 出力: `SafeMatchOutcome{decision: AUTO|REVIEW|UNRESOLVED, top: Option<&TmdbCandidate>, reasons: Vec<&'static str>}`。
- `rules_one`（凍結 rules-1）は FINAL の対象外。契約では記録しない（§9 Q4）。Jev は使わない。
- **PF0C は「どちらの matcher が FINAL 対象か」を名指ししていない**（docs 内 grep 結果）。契約は両方を出力し、どちらを PF0B の `Machine` に写すかを manifest で **事前宣言**させる（§9 Q1）。

### 1.5 TMDB response の最小保存

スコアリング・判定・候補生成が読む field は `TmdbSearchMovie/Tv` のうち: `id, title|name, original_title|original_name, release_date|first_air_date, original_language, poster_path, overview`（`overview`/`poster_path` は `TmdbCandidate` に **コピーされるだけ**で、`rules_core` もスコアも参照しない — grep ベース。実装時に consumer-field audit テストで確認）。`rules_v1` は `id, title, original_title, release_date` のみ。`vote_average`(float)/`genre_ids` はスコアに未使用。応答トップの `total_results` は未使用だが、truncation（20 件超）の検知に有用。

### 1.6 PF0B への変換に要る情報

`Unit{key, machine, gt}`。`machine` は `Auto(Option<(media_type, tmdb_id)>) | Review | Unresolved | Missing`。`key` は ledger の `evaluation_unit_key`（`work:{id}`）と一致し、`membership_commitment` が全 946 件で再計算される。→ machine 側 artifact は **key・machine のみ**を供給し、`gt` は blind GT 側から別経路で結合する（machine artifact は GT を一切含まない）。

---

## 2. 設計原則

1. **Replay 可能**: evaluator は TMDB に触れず、保存済み capture だけから candidate set と matcher output を再導出して一致を検証できる（ネットワーク非依存の再検証）。
2. **事前凍結**: run 前に `FINAL_MACHINE_RUN_MANIFEST` を確定し、その sha256 を audit freeze に固定する。run 中に manifest を変更不可。
3. **1 回だけ**: run には単調な `attempt` と排他 lock。2 回目の起動は manifest に宣言済みの resume 規則（§6.3）以外は拒否。
4. **追記専用 + hash chain**: 各 JSONL は PF0E ledger と同じ `prev_commitment / record_commitment` 方式。最後に `ARTIFACT_INDEX`（全ファイル sha と chain head）を封印。
5. **secret ゼロ**: API key およびそこから派生する値（hash・fingerprint・長さ）を一切残さない（§5）。
6. **GT 非接触**: machine artifact に GT・label・verdict を含めない。generator は GT ストア/H1/Jev を import しない。
7. canonical JSON は既存 `pr4-canonical-json-1`（整数のみ・キー順・null 保持）を再利用。float は禁止し、必要なら文字列表現。

---

## 3. Artifact 構成

保存先（案）: `CineMantis-audit\pr4\pf0f\final_run\<run_id>\`（H2 の `pf0e` 配下とは別ディレクトリ。生成側のみ書き、evaluator は read-only で読む）。

```
final_run/<run_id>/
  MANIFEST.json                 # FINAL_MACHINE_RUN_MANIFEST（run 前に凍結・sha を audit freeze へ）
  unit_input.jsonl              # UNIT_INPUT       1行/unit（946行）
  tmdb_request.jsonl            # TMDB_REQUEST     1行/論理call（再試行は attempts[] に内包）
  tmdb_response/<sha256>.json   # TMDB_RESPONSE_CAPTURE 本体（content-addressed・projection）
  tmdb_response_index.jsonl     # request_id → capture sha / status
  candidate_set.jsonl           # CANDIDATE_SET    1行/unit
  matcher_output.jsonl          # MATCHER_OUTPUT   1行/unit（rules-safe と shadow を両方）
  evaluation_unit.jsonl         # PF0B Unit 投影（key + machine のみ・GT なし）
  run_log.jsonl                 # イベント/失敗ログ（追記専用）
  RUN_PROVENANCE.json           # 実行後に確定する環境情報
  ARTIFACT_INDEX.json           # 全ファイルの sha256・行数・chain head（封印）
```

### 3.1 FINAL_MACHINE_RUN_MANIFEST（run 前に凍結）

- `contract_version`、`run_id`（UUID）、`attempt_policy`（max_attempts=1、許可する resume 規則）。
- **入力の固定**: `tranche_commitment`（PF0E の close record / ledger head sha / unit 数=946）、`payload_set_sha256`（各 `matcher_input_sha256` を key 順に連結した digest。値は読まなくても ledger から得られる）、`membership_commitment`（`pr4-pf0a-membership-1`）。
- **実装の固定**: `code_commit`（40 hex・clean tree 必須）、`evaluator_tool_sha256`、`generator_binary_sha256`、`cargo_lock_sha256`、`rustc_version`、各 matcher 識別 (§4.5)。
- **TMDB request template**: base URL、endpoint、固定 param（`language=ja-JP`, `include_adult=false`）、page=1、year param 名、encoder 名、timeout、`MAX_HTTP_ATTEMPTS`、backoff。**api_key の記述なし**。
- `final_matcher_binding`: PF0B `Machine` へ写す matcher（`rules_safe` | `rules_tags_shadow`）。もう一方は diagnostic 扱い。
- `policy_ref`: `pr4_final_policy.json` の sha256（PF0C を変更しないことの固定）。
- `generation_window`: 許容開始/終了時刻（UTC）— 時刻は provenance であって判定入力ではない。

### 3.2 UNIT_INPUT（1 行/unit）

`evaluation_unit_key`、`matcher_input_sha256`（ledger の値）、`payload_recomputed_sha256`（generator が payload から再計算）、`payload_match: bool`、`ledger_seq`、`evidence_class_at_enrollment`、`local_evidence_sha256 / embedded_evidence_sha256 / decision_evidence_sha256`（PF0E `evidence_hashes` を再計算し ledger の enrollment_context と一致確認）、`derived`（下記）、`query_plan`（全 input 分）。

`derived` は **payload 本文を複製せず**、matcher が見た値の最小要約: `search_inputs[]{source,title,year}`、`media_kind`、`filename_year`、`embedded_year`、`skips_tmdb`。title は検索語として request 側にも出るため再検証に必要。`planned_logical_calls`（`planned_logical_calls()` の値）を持ち、実際の call 数との一致を検証する。

不一致（`payload_match=false` など）は unit を `technical_failure(INPUT_MISMATCH)` にし、run 全体を TechnicalFailure 候補にする（§6）。

### 3.3 TMDB_REQUEST（1 行/論理 call）

`request_id`（`{key}#{input_index}.{kind}.{variant}` 例 `work:12#0.movie.year`）、`evaluation_unit_key`、`input_index`、`source`(legacy|embedded)、`kind`(movie|tv)、`variant`(primary|retry_without_year)、`sequence`（unit 内の実行順）、`canonical_request`:
`{method:"GET", path:"/3/search/movie", params:{include_adult:"false", language:"ja-JP", query:<decoded UTF-8>, year:"1999"}}`（key 昇順、**api_key 無し**）、`canonical_request_sha256`、`attempts[]`（各 `{n, started_at_utc, outcome: response|timeout|connect|interrupted|other, http_status?, duration_ms}`）、`final_status`(ok|http_error|network_error|decode_error|not_attempted)。

- URL は保存しない（api_key が入るため）。`query` は decode 済み文字列で保存し、エンコード済みの wire 形式は `wire_query_sha256`（api_key を**除いた**クエリの sha）だけ残す。
- production と同じ呼び出し数であることを `planned_logical_calls` と照合。再試行は `attempts[]` 内に入れ、論理 call 数に数えない。

### 3.4 TMDB_RESPONSE_CAPTURE（content-addressed）

- 保存するもの（既定・**projection tier**）: `request_id`、`http_status`、`received_at_utc`（provenance、判定入力ではない）、`results[]`（返却順を保持、各要素 = `{rank_in_response, id, title|name, original_title|original_name, release_date|first_air_date, original_language, poster_path, overview_sha256, overview_len}`）、`total_results`。
- **float(`vote_average`) と `genre_ids` と overview 本文は保存しない**（スコア・判定に未使用・canonical JSON が float 不可・TMDB コンテンツの再配布量を最小化）。`capture_sha256` = projection の canonical JSON の sha256。
- `raw_body_sha256` と `raw_body_bytes`（受信した生ボディの sha と長さ）は記録する。ただし生ボディが無いので第三者が再計算はできない — あくまで capture 時の commitment。
- **raw tier（任意）**: 生ボディを gzip で封印保存する場合は manifest で事前宣言。必要性: projection が consumer-field audit を満たせば不要（推奨 = 保存しない）。サイズ見積もり: 1 レスポンス ≲ 数十 KB（最大 20 件×overview）、論理 call ≈ 946 × 2〜5 ≒ 2–5k 件 → raw ≈ 数十〜100 MB 規模、projection ≈ 数 MB。個人情報: TMDB 検索応答に個人情報は無い（作品メタ）。secret: ボディに api_key は含まれないが、**ヘッダ/URL は保存しない**ので問題なし。TMDB の利用規約上の再保存可否は未確認（§9 Q3）。

### 3.5 CANDIDATE_SET（1 行/unit）

2 系統（`legacy` と `combined`）それぞれについて:
- `all[]`（pre-rank の挿入順）: `{ord, tmdb_id, media_type, source_request_id, rank_in_response, query_source(legacy|embedded|both), title, original_title, year, original_language, confidence, reasons[]}`。
- `ranked[]`（post `merge_and_rank`）: `all` の `ord` への参照 + `final_rank`（1..≤10）。tie-break 規則 `STABLE_SORT_CONFIDENCE_DESC_THEN_INSERTION_ORDER` と、`threshold=40`, `cap=10` を manifest/各行の `ordering_rule` に宣言。
- `merge` の動き（同一 identity で confidence の高い方を採る、`both` 化）を再現できるよう、`merged_from[]`（採用元の `ord`）を持つ。
- `candidate_set_sha256`（canonical JSON）。

identity = `(media_type, tmdb_id)`（PF0B `Gt::Positive(String,i64)` と同形）。

### 3.6 MATCHER_OUTPUT（1 行/unit）

`rules_safe` と `rules_tags_shadow` の両方に `{decision: AUTO|REVIEW|UNRESOLVED, top: {media_type,tmdb_id}|null, top_confidence, reasons[], input_candidate_set: "legacy"|"combined", input_candidate_set_sha256}`、`embedded_evidence`（5 項目）、`parsed_year_hint`。加えて `final_matcher` と `pf0b_machine`（§3.7 の写像結果）、`technical_status`。

### 3.7 evaluation_unit（PF0B 投影・GT なし）

`{evaluation_unit_key, machine: {kind: AUTO|REVIEW|UNRESOLVED|MISSING, identity?: {media_type,tmdb_id}}, technical_status}`。

写像（`final_matcher` の出力を使う）:

| matcher 結果 | PF0B `Machine` |
|---|---|
| AUTO + top あり | `Auto(Some((media_type, tmdb_id)))` |
| AUTO + top なし | `Auto(None)`（無効出力 → evaluator が TechnicalFailure） |
| REVIEW | `Review` |
| UNRESOLVED（`skips_tmdb` の 0 call を含む） | `Unresolved` |
| technical_status ≠ ok | `Missing` |

### 3.8 RUN_PROVENANCE（run 後に確定）

`run_id`、`started_at_utc / finished_at_utc`、`code_commit`（manifest と一致必須）、`binary_sha256`、`cargo_lock_sha256`、`rustc/target triple`、OS 名/版、`locale`（`LANG` 等の allowlist 値）、`timezone`、env allowlist（**値を記録してよい key のみ。`*KEY*`/`*TOKEN*`/`*SECRET*` は key 名自体も記録しない**）、matcher/parser/schema バージョン (§4.5)、unit 数/call 数/失敗数（件数のみ）、`attempt`、`manifest_sha256`、`artifact_index_sha256`。

---

## 4. 再検証手順（evaluator 側）

1. `ARTIFACT_INDEX` の各 sha と hash chain を検証。`manifest_sha256` が audit freeze と一致。
2. `membership_commitment(unit_input keys)` が凍結値と一致。
3. 各 unit: `matcher_input_sha256` が ledger と一致。query plan を `query_plan_for` で**再導出**し記録と一致。
4. capture から `fetch_candidates` 相当を **offline で再実行**（TMDB を呼ばない replay 関数）して `candidate_set_sha256` と一致。
5. `rules_*` を再実行して `matcher_output` と一致。
6. 写像を再計算して `evaluation_unit` と一致。
7. 全 unit が揃った後に初めて GT を結合して PF0B `evaluate()` に渡す。

### 4.5 バージョン識別

コード上に matcher バージョン定数は無い（`STATE_SCHEMA_VERSION="cm-prematch-2"`、`PAYLOAD_SCHEMA_VERSION`、`CANONICALIZATION_VERSION`、`THRESHOLD_AUTO=75/CANDIDATE=40`、`YEAR_TOLERANCE`、`RULES_V1_VERSION` のみ）。→ 契約では **matcher の識別 = `code_commit` + 対象ソースファイルの sha256**（`metadata_matcher.rs`, `title_parser.rs`, `container_tags.rs`, `rules_v1.rs`, `tmdb_client.rs`, `commands/tmdb.rs` の `fetch_candidates` 周辺）と、上記定数の値を manifest に記録する。バージョン定数の新設は §9 Q5。

---

## 5. Secret / PII 規則

- artifact に書いてはいけないもの: API key、key の hash/fingerprint/長さ、`api_key=` を含む URL、HTTP ヘッダ、環境変数の値（allowlist 外）、`settings` DB の内容、file_path（payload 由来でも出さない）。
- generator は書き込み前に全出力を走査する **secret scanner**（実 key 値と `api_key` パターン）を通し、ヒットしたら run を中止（TechnicalFailure）。scanner は key 値をメモリ内でのみ比較し、ログにも出さない。
- artifact に含まれる作品タイトル/検索語は、payload 由来のローカルファイル名情報を含み得る（個人のライブラリ名）。外部共有しない前提で audit ディレクトリに置く。レポートには件数のみ（PF0B の方針と同じ）。

---

## 6. 技術的失敗 / 無効出力の扱い

### 6.1 分類（unit の `technical_status`）

| status | 意味 | PF0B |
|---|---|---|
| `ok` | 全 planned call が成功（0 call の `skipped_no_title` を含む） | 通常 |
| `skipped_no_title` | `skips_tmdb` により 0 call（正規） | `Unresolved` |
| `partial_call_failure` | 一部の call が最終失敗（再検索失敗・片方 kind 失敗を含む） | `Missing` |
| `total_call_failure` | 全 call 失敗（production なら Err） | `Missing` |
| `decode_error` | 応答が decode 不能 | `Missing` |
| `input_mismatch` | payload/ledger/evidence hash 不一致 | `Missing` |
| `invalid_output` | AUTO なのに top なし、replay 不一致など | `Auto(None)` / `Missing` |
| `internal_error` | panic・書き込み失敗 | `Missing` |

production と違い、**year なし再検索の失敗も `partial_call_failure`** とし、握りつぶさない。失敗でも `tmdb_request` の attempts と `run_log` に原因種別（kind のみ。URL/ボディ断片は不可）を残す。

### 6.2 run レベル

`Missing` が 1 件でもあれば PF0B は TechnicalFailure（`input.run_failure` も併用）。artifact は **失敗を含めたまま封印**し、失敗 unit を黙って落とさない。

### 6.3 再試行/Resume（要決定 §9 Q2）

「1 回だけ生成」と矛盾しないよう、**事前宣言した 1 種類のみ**許可する案: 失敗 unit に限り、同じ manifest・同じ request template で 1 回だけ追補（`attempt=2`）。成功 unit は上書き不可・再実行不可。追補後も失敗が残れば TechnicalFailure。追補の有無は FINAL の結果を見る前に確定している必要がある。

---

## 7. Single-run 強制

- `run_id` 単位の排他 lock ファイル（`O_EXCL` 作成）。manifest sha が audit freeze と一致しない限り起動しない。
- `ARTIFACT_INDEX` が存在する run_id への再書き込み拒否。
- 起動前チェック: close record 存在・946 件・working tree clean・HEAD == manifest.code_commit。
- TMDB 呼び出しは generator の単一経路のみ。テストビルドの `cfg` で live HTTP を禁止（synthetic では mock transport）。

---

## 8. Synthetic test plan（実データ不使用）

すべて synthetic payload + mock TMDB transport（fixture JSON）。ネットワークなし、DB なし。

| # | テスト | 期待 |
|---|---|---|
| T1 | payload → query plan の再導出（legacy のみ / embedded 追加 / embedded 同一で非追加 / skips_tmdb） | `planned_logical_calls` と一致 |
| T2 | request に `api_key` が現れない（canonical_request、attempts、log、全 artifact を走査） | ヒット 0 |
| T3 | secret scanner: 偽 key を意図的に混入 | run 中止 |
| T4 | 候補 ordering: 同点候補の返却順入替で `ranked` 順が追従、`ordering_rule` が記録 | replay 一致 |
| T5 | cap 10 / threshold 40 境界、merge で `both`/confidence 高い方採用 | replay 一致 |
| T6 | replay: capture のみから `candidate_set_sha256` / matcher output が再現 | 一致 |
| T7 | year なし再検索失敗 → `partial_call_failure`（production の握りつぶしを再現しない） | `Missing` |
| T8 | movie 成功/tv 失敗、HTTP 4xx、timeout×3、decode error | 各 status |
| T9 | `skips_tmdb` unit → 0 call・`Unresolved`（失敗ではない） | ok |
| T10 | AUTO + top なし | `Auto(None)` → PF0B TechnicalFailure |
| T11 | PF0B 写像: AUTO/REVIEW/UNRESOLVED/Missing 4 値、`membership_commitment` 一致 | evaluate が通る |
| T12 | hash chain 改ざん（行の編集/削除/並び替え/ファイル差替え）検出 | 検出 |
| T13 | single-run: 2 回目起動拒否、封印後の追記拒否、manifest sha 不一致拒否 | 拒否 |
| T14 | float を含む応答（`vote_average`）→ projection から除外、canonical 化が通る | OK |
| T15 | overview 変更のみの応答差で candidate/matcher 出力が不変（consumer-field audit） | 不変 |
| T16 | source-scan: generator が GT/label/verdict/H1/Jev/DB 書込 API を参照しない | 参照 0 |
| T17 | determinism: 同一 capture から 2 回 replay → bit 一致、locale/TZ を変えても一致 | 一致 |
| T18 | PF0C policy json の sha が manifest の `policy_ref` と一致、policy 値不変 | 一致 |

---

## 9. 未決の設計論点（unresolved）

| Q | 論点 | 推奨 |
|---|---|---|
| Q1 | FINAL で評価する matcher は `rules_safe` か `rules_tags_shadow` か（PF0C は名指ししていない。shadow は「昇格候補」、safe は現 production）。manifest で事前宣言が要る | 監査者判断。両方出力は確定、binding の宣言を run 前に凍結 |
| Q2 | 技術的失敗の追補 run を許すか（§6.3） | 失敗 unit のみ 1 回の追補を事前宣言する案。許さない場合は 1 件の失敗で TechnicalFailure |
| Q3 | raw TMDB body を保存するか（既定は保存しない）。TMDB 利用規約上の保存可否、overview 本文を保存しない判断の妥当性 | projection のみ。raw は保存しない（hash のみ） |
| Q4 | `rules_one`（凍結 rules-1）を FINAL artifact に併記するか | 今回は対象外（決定を増やさない） |
| Q5 | matcher/parser バージョン定数を新設するか（現状は無い）。code_commit + ソース sha で代替可 | 代替で十分。定数新設は production 変更になるため避ける |
| Q6 | consumer-field audit（overview/poster_path/vote_average/genre_ids が判定に未使用）は grep ベースの確認に留まる。実装時に静的/動的に確定が必要 | T15 で確定。未使用でなければ projection に追加 |
| Q7 | TMDB の返却は時間で変わる。generation window の長さと、取得時刻をどう扱うか（判定入力でないが監査対象） | 946 件を単一ウィンドウ（数時間以内）で実行し、`received_at_utc` を記録 |
| Q8 | rate limit（TMDB 429）時の扱い。現行 client は 429 を再試行しない（HTTP error で即 Err） | FINAL では 429 を `http_error` として失敗扱い。pacing（例 260ms 間隔）を manifest に固定するか |
| Q9 | 保存先ディレクトリと、audit freeze へ固定する値（manifest sha / artifact index sha）の受け渡し経路 | 監査者と合意 |
| Q10 | `evidence_class` を generator がどこから得るか（payload 外。ledger の enrollment_context が正） | ledger から読み、ledger の他の値と照合 |

---

## 10. この設計がやらないこと

実装、H2 ledger/payload の読み出し、TMDB live call、GT の読み出し、PF0C policy の変更、production コードの変更、H1/Jev の利用、メイン checkout の変更。
