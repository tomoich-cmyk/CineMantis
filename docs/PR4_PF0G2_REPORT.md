# PR4-PF0G2 REPORT — reviewer packager / typed API / loopback server / UI

状態: **実装完了・FREEZE 候補（未 commit）**。FINAL = NOT_RUN / holdout sealed / H2 は日次蓄積中のまま。
base: `1a6266d9869d7dc3bf8556850312c85676ebe7d3`（branch `claude/pr4-pf0g2-210938`）。schema anchor は同 commit の契約 §17・schema JSON・addendum。PF0G1 実装（`pr4_blind_gt.rs`）は eeeb573 から **byte 不変**。

## 触れていないもの
H2 の ledger・payload・daily runner、プライマリ checkout、main / feat/awards-festivals、sealed holdout、production DB、live TMDB、実 API key。PF0F1 / PF0F2 は `git show` で読むだけ。入力は synthetic の凍結 projection のみ。通信は 127.0.0.1 の loopback だけ。新しい依存なし（Cargo.toml / Cargo.lock 不変）。

## 追加物
| ファイル | 内容 |
|---|---|
| `services/pr4_pf0g2.rs` | packager（`build`）、A8 leak scan（`a8_leak_scan`）、UI package pin（`ui_package_sha256` / `make_pins`）、本体アプリ probe（`ProcessProbe`）、PF0I handoff（`pf0i_handoff` / `pins_from_ledger`） |
| `services/pr4_pf0g2_server.rs` | 型付き DTO の `Api`、TMDB transport（`HttpTmdb`・`wire_target`）、loopback `Server` |
| `services/pr4_pf0g2_ui/index.html` | 静的 UI（日本語、textContent のみ、a11y 属性つき） |
| `services/pr4_pf0g2_tests.rs` | 30 test |
| `services/mod.rs` | 登録の 4 行だけ |

## 仕様と実装の対応
- **PF0F の frozen projection だけが入力**: `parse_projection` が `REVIEWER_PROJECTION` を厳格検証（キー集合の完全一致、(media_type,tmdb_id) 厳密昇順、`shown_set_sha256` 再計算）。`rank / score / source / decision / reasons / confidence / jev / work_id / display_title` のどのキーを足しても拒否し、失敗時は出力を残さない。overview・title は projection の値をそのまま写す（packager に通信コードは無く、ソース走査で固定）。`canon` が PF0E の `canon` と一致することも test で固定。
- **reviewer package と sealed side の分離**: reviewer 側は `manifest.json` + `units/` のみ（`check_reviewer_package` を build 直後と起動時に実行）。`unblind_map.json` / `build_secret.json` / `build_attestation.json` は別ディレクトリ（入れ子禁止）。unit には `tmdb_id` / `media_type` / `release_date` / work id が無い。candidate は `candidate_token`（HMAC、契約 §17 R4）。unblind map の全 token を HMAC で再計算して一致を確認。
- **表示しないもの**: API 応答（DTO）に rank・score・confidence・reasons・source・machine decision・`display_title`・`work:`・work_id・`candidate_count`・`unit_salt`・`overlap_digests`・`position` が出ない。進捗は `{done, remaining, deferred, session_elapsed}` のみ（schema の allowlist と一致）。候補 0 件でも自動 none にならず、none は検索 1 回以上が必要（API 側でも拒否）。
- **A8**: (1) 開始前 scan = `a8_leak_scan`（package・UI・禁止リテラルを走査し、決定的な report の SHA を `a8_leak_scan_report_sha256` として pin。**1 件でも漏洩があれば STOP**）。(2) 実行時 scan = 全 API 応答の (type, key path 集合) を allowlist と照合し、外れた応答は出さず violation として数える（`runtime_a8_report`）。
- **pin**: `make_pins` は `ui_package_sha256` を実際の UI から計算する。init と seal で **8 pin すべて**が完全一致を要求され、1 項目ずつ変えた 8 通りが全て `PinMismatch(その pin 名)`。a8 report SHA が不正なら seal 前に拒否。`Api::attach` は配る UI の SHA が init pin と違えば起動拒否。
- **本体アプリ稼働中は起動拒否**: `Api::attach(…, startup_probe)` と `Store` の全 command（`MainAppRunning`）。process 一覧が取れなければ「動いている」とみなす（fail closed）。
- **pick_other の TMDB 検証**: 3 段（lease・形式・非候補 → 通信 → 再確認）。検証結果は **server が保持**し、submit は client の値を使わない。候補 ID は通信前に `TargetIsCandidate`。別 id 返却・adult・404・429・500・timeout・不正 body は全て提出不可（ledger に何も残らない）。応答本文は audit 側 `tmdb_cache/<sha>.json` に保存。
- **TMDB 通信**: `wire_target` は production `tmdb_client.rs` の URL template（movie/tv search・detail）と `urlencoding` 規則を、ソース照合 + 実 HTTP loopback で受信した request-target との一致で検証（PF0F2 の方式）。API key は URL にだけ使い、DTO・ledger・cache・Debug に出ない。loopback 以外へは `production()` でしか向けられず、テストは呼ばない。
- **seal / errata / PF0I**: seal event の `event_commitment` = `review_seal_sha256`（test で ledger 末尾と一致）。seal 後は API が `Sealed`(410)、`events.jsonl` は errata 追加後も byte 不変、`promotion_impacting` で `Invalidated`。API に seal / unblind / errata の route は無い（404 とソース走査）。
- **HTTP**: 127.0.0.1 固定、起動ごとのランダム token（`X-Review-Token`、UI は `/?t=`）、Host / Origin 検査、body 64KB 上限、CSP・no-store。

## PF0I 確定契約への整合（追補）
`unblind_map.json` は `pr4-pf0i-unblind-bundle-1`（`bundle_version` / `build_secret_hex` / `units[{review_unit_id, work_key, candidates[{candidate_token, media_type, tmdb_id}]}]`、未知キーなし）で出力し、`unblind_map_sha256` はその実 bytes の SHA。`build_secret_commitment = SHA256(ASCII(lowercase hex))`（0x・空白・domain なし）。別ファイルの `build_secret.json` は廃止。test で固定。

## 契約 parity（自動テスト）
`the_authoritative_docs_are_the_ones_at_commit_1a6266d…`: schema / 契約 / addendum の SHA-256（LF 正規化）を 1a6266d の値に pin し、`tool_sha256()` = PF0G1 freeze の値も pin。`the_implementation_matches_the_schema_anchor_semantically`: unit / manifest の key・label mapping・defer 理由・canonicalization・api allowlist の名前と progress の項目・8 pin 名・§17 の語を schema/契約から機械照合。全 DTO key が schema の `forbidden_anywhere` に当たらないことも走査。

## 結果
`cargo test --lib`: **611 passed / 0 failed / 12 ignored**（PF0G1 時点 581 + PF0G2 30、ignored は既存）。`pr4_pf0g2` を 3 回連続実行して安定。`cargo check --lib` OK。UI の script は `node --check` で構文 OK（ブラウザ実機は未）。

## Hard STOP の確認
test failure なし / schema anchor との意味的不一致なし（上記 parity）/ 漏洩なし（canary を含む全応答・ledger・package・UI を走査）/ reviewer package への build_secret・unblind_map 混入なし / seal・errata・pin 契約違反なし / production・H2・main 変更なし / 破壊的操作なし。

## PF0I への handoff 可否
**可（1 点だけ定義待ち）**。取り出し口 `pf0i_handoff(store, ledger_dir, sealed_dir)` が `authoritative_membership_commitment` / `pf0h_final_manifest_sha256` / `review_seal_sha256` を返す。seal 前は拒否。sealed の `unblind_map.json` と `build_secret.json` は init の pin（`unblind_map_sha256` / `build_secret_commitment`）と一致しなければ拒否。残件 D4: 前 2 値は PF0H から受け取る入力として載せるだけ（64 hex 検査のみ）。PF0H/PF0I 側での定義が決まれば packager で再計算して突合できる。

## 残る LOW（`DEFERRED_FIXES.md`、10 件）
CSP の unsafe-inline、実ブラウザ E2E/a11y、reviewer bin（PF0G3）、D4、Store の Send 制約による factory 方式、`Store::pins()` 不在、reviewer 側の自動 retry なし、`multi` 検索なし、946 件フル工程は PF0G4、提示順の文言差。

## 未了（Human Gate）
commit / push はしていない。commit 候補は新規 5 + 変更 1（`mod.rs` +4）+ docs 3（`PR4_PF0G2_REPORT.md` / `PR4_PF0G2_FREEZE.txt` / `DEFERRED_FIXES.md`）。ファイル SHA-256 は `docs/PR4_PF0G2_FREEZE.txt`。
