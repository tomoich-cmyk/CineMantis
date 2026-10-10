# PR4-PF0F2 REPORT — TMDB Transport / Wire-Parity Gate

base: PF0F1 freeze `5a31b5fa513243f7cbe6b411d19b7ca5dd1beaaf`（branch `claude/pr4-pf0f2-e199e0`、main H2 worktree には merge しない）
状態: **CLOSED 候補（live smoke なし）**。FINAL=NOT_RUN / holdout sealed / policy 未 freeze のまま。監査の判定待ちで **commit していない**。

## 触れていないもの
H2 実データ / production DB / live TMDB / Jev / main H2 worktree / 実 API key。key は fake canary（`PF0F2-FAKE-KEY-…`）のみ。ネットワークは `127.0.0.1` の loopback だけ。

## production 変更（visibility のみ・1 行）
`services/tmdb_client.rs`: `fn urlencoding` → `pub(crate) fn urlencoding`。ロジック変更なし。
`services/mod.rs`: `#[cfg(test)] pub mod pr4_pf0f2;` を追加（test ビルドだけ）。Cargo.toml / Cargo.lock は無変更。

## 追加物
`services/pr4_pf0f2.rs`（test-only、19 test）: wire builder `wire_target`、loopback server `Loopback`、実 HTTP transport `WireTransport`（`pr4_pf0f::Transport` の実装）、source tripwire `check_source`、policy parity `policy_parity`。PF0F1 の generator（`process_unit` の retry/backoff）をそのまま使い、transport だけを loopback の実 HTTP に差し替えて検証した。`pr4_pf0f.rs` は無変更。

## 一致の示し方（正直な限界つき）
production の base URL は const で差し替え不能、URL は `search_movie` / `search_tv` 内に inline で組まれている。ロジックを触らない制約上、**production の client を loopback に向けることはできない**。代わりに:
1. production ソースから `format!` テンプレートを機械抽出し、production の `urlencoding` で描画した URL を「production の URL」とする。
2. test 側 builder の出力・loopback server が受信した request-target を、その URL と**バイト一致**で比較（movie/tv × 年あり/なし × 19 種の query: 空白・日本語・`&=?#%+`・絵文字・全角・`%2B` 等）。
3. 上記テンプレートと retry/timeout 定数を含む production 断片を SHA-256 で pin（tripwire）。**source SHA の一致は意味的同一性の証明ではなく tripwire**。意味的一致は 2 の wire 比較が担う。

## Acceptance 対応表
| 項目 | 結果 | test |
|---|---|---|
| movie / tv endpoint 一致 | OK | `wire_builder_is_byte_identical_…`, `loopback_receives_exactly_…` |
| year あり/なし（movie=`year`、tv=`first_air_date_year`） | OK | 同上 |
| language=ja-JP, include_adult=false | OK（他の値は STOP） | 同上, `unknown_or_altered_requests_stop_…` |
| query の percent-encoding（production `urlencoding` を直接呼ぶ） | OK、受信側で decode すると元の query に戻る | `…query_round_trips`, `production_urlencoding_is_the_one_…` |
| request 順序（param 順 `api_key,query,language,include_adult,year`／unit 順／attempt 順） | OK | `artifact_request_sha_equals_…` |
| API key は送信に使うが artifact に残らない | OK（artifact 全文・Debug・`api_key=`・loopback addr を走査して 0 件） | `the_api_key_is_sent_but_never_left_…` |
| artifact の request SHA = 実送信内容の SHA | OK（受信 target から復元した canonical request の SHA を attempt 単位で照合）。送信を改ざんする transport は全件不一致として検出 | `artifact_request_sha_…`, `a_transport_that_sends_something_other_…` |
| 429 / 5xx(500,502,503,504) / timeout / 切断 / 接続拒否 | OK: 最大 3 attempt、pause は [400, 800]、最後の後は待たない、400/401/404 は 1 回のみ | `http_429_…`, `http_5xx_…`, `non_retryable_…`, `a_timeout_…`, `a_dropped_…`, `connection_refused_…` |
| Retry-After（ignore / honor_capped とその cap） | OK | `retry_after_…` |
| manifest 固定の policy | OK: 改ざん（max_attempts / concurrency）は送信前に拒否。manifest の max_attempts / backoff / timeout_secs / concurrency は production 定数（3 / 400·n / 30s）とソースから抽出して照合 | `a_tampered_policy_…`, `production_retry_and_timeout_constants_…` |
| concurrency=1 | OK: server 側で同時 in-flight の最大値を計測して 1（timeout 後も旧接続が残ったまま次を送らない） | 各 run の `max_inflight` |
| 未知/変更された URL builder は STOP | OK: 12 種の変異（language/adult/param 順/year 名/encode/定数/base/status retry/新 builder/新 host 等）を全て検出。CRLF 化だけでは落ちない | `tripwire_stops_on_any_change_…`, `production_source_matches_the_pinned_fragments` |
| 未知の CanonReq（path・param・year 形式・key 形式）は STOP し何も送らない | OK | `unknown_or_altered_requests_stop_…` |

## 監査に見てほしい点（発見事項）
1. **policy と production の差（意図的・文書化）**: production の `send_with_retry` は接続系エラーのみ再試行し、HTTP 429/5xx は再試行せず `Ok(response)` を返す。PF0F1 の manifest policy は 429/5xx も再試行する。FINAL run は policy が決めるので差は仕様どおりだが、「FINAL は production より多く再送し得る」ことは事実。`retries_http_status=false` を test で pin 済み。
2. **timeout は production client を直接は検証していない**: production の 30 秒 timeout は定数の照合（manifest `timeout_secs`=30 = ソース）まで。loopback の timeout test は短い timeout（150 ms）の test 用 client で、分類は production の `send_with_retry` と同じ if 連鎖を写した。`send_with_retry` を `pub(crate)` にすれば production 実体を叩けるが、許容範囲（URL/request builder の visibility のみ）外なので**行っていない**。必要なら別途承認を。
3. **Windows の接続拒否は遅い**: connection-refused test は OS の再試行で約 6 秒かかる（timeout を 8 秒にして分類を `connect` に固定）。
4. manifest の `tmdb_request_template.page=1` は説明用で、production も wire も `page` を送らない（TMDB の既定が 1）。
5. detail / credits / person / poster の URL builder は generator の対象外。inventory に含め、増減すれば STOP する（wire parity は search のみ証明）。

## 監査裁定の反映（PASS WITH 1 HIGH CLARIFICATION → deployment requirement として事前開示）
```
production_tmdb_client_executed_against_loopback = false
wire_parity_method = production format-template extraction + production urlencoding + loopback received request-target comparison
current_production_retry_policy_equal_to_final = false
final_transport_policy_is_binding_deployment_requirement = true
promotion_requires_deployed_transport_parity = true
```
- FINAL_DEPLOYMENT_BINDING: matcher `rules-tags-shadow-2` + frozen PF0F query/candidate semantics + frozen PF0F `transport_policy`（retry / backoff / timeout / pacing / concurrency）の bundle。matcher 単体を現 production transport に載せての昇格はしない。昇格時は deployed transport が FINAL 評価時の policy と一致することを Release Gate で確認する（現 production は 429/5xx を再試行しない）。
- 30-second wall-clock production timeout behavior itself was not exercised（定数 30 秒の一致と、短縮 timeout loopback での分類/制御のみ）。`send_with_retry` は `pub(crate)` にしない。
- binding scope = search movie/tv のみ。detail / credits / person / poster = OUT OF SCOPE（live poster fetch はしない前提）。
- live TMDB smoke: NOT REQUIRED。

## 結果
`cargo test --lib`: **591 passed / 0 failed / 13 ignored**（PF0F1 の 572 + PF0F2 の 19。ignored は既存 12 + pin 再生成用 `dump_fragment_pins` 1）。`pr4_pf0f2` を 3 回連続実行して安定（各 約 6 秒）。`cargo check --lib`（非 test ビルド）も通る。

## 未了（監査の判断事項）
- commit していない。freeze 値は `docs/PR4_PF0F2_FREEZE.txt`。
- live smoke は行っていない（CLOSED に不要との指示）。
