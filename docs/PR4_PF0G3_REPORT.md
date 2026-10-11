# PR4-PF0G3 REPORT — browser E2E / CSP / reviewer binary（FINAL ENGINEERING）

base: `954ac81d7c3f8acb45a6d60b162ad876918d3b9a`（G/H/I integration）。触れていないもの: main / feat/awards-festivals、primary H2 checkout・H2 ledger/payload、production DB、H1・GT・sealed holdout、live TMDB、実 API key。全て synthetic fixture + 127.0.0.1 loopback。

## A. 実ブラウザ E2E
- engine: Chrome/152.0.7977.130（Claude desktop の built-in browser pane）、Windows 11 10.0.26200。viewport 375x812（mobile）。
- fixture: synthetic 6 unit（manifest `4a41e932…cd10` は `browser_e2e_result.json` に全 SHA）。同一 seed・同一入力で独立 4 回 build して unit / token / 表示順が一致（決定論 shuffle の実機確認）。
- driver: `docs/pf0g3_evidence/browser_e2e.js`（page 内で実 DOM click / KeyboardEvent）。fixture server: `cargo test --lib pf0g3::browser_fixture_server -- --ignored`（手動・`CM_PF0G3_FIXTURE_OUT`）。
- 結果（`docs/pf0g3_evidence/browser_e2e_result.json`）: **85/85 PASS**、JS error 0、CSP violation はちょうど意図した 5 probe のみ。
  - 表示順 = package 順（再提示・QC repeat 含む全 unit）、候補 key = allowlist（`candidate_token,title,original_title,year,overview,poster_path,original_language`）。`/api/next` に `tmdb_id` / `media_type` / `c1` / `c2` / `work_id` / rank / score 等が無い。応答・描画テキストに build_secret・unblind・audit・canary・key literal が無い。
  - confirm = `candidate_token` のみ（tmdb_id / media_type は null）。pick_other = 検索→preview 検証後のみ有効、tmdb_id+media_type・token なし。none = 検索 1 回以上で有効。defer = 理由のみ（`other_with_note` はメモ付き）。attestation なしの開始は拒否（role=alert）。
  - keyboard: 候補は `role=radio` `tabindex=0`、Space/Enter で選択（`aria-checked`）。mobile 幅で横 scroll なし。
  - network: page が出す fetch は同一 origin `/api/*` のみ。poster は取得しない（`img-src 'none'`、DOM に img なし）。TMDB 側 fake の受信は loopback の `/3/search|detail` 4 件のみ。
  - sealed（operator が in-process で seal → server 再起動）: `session/start`・`submit` = 410 sealed、`seal|unblind|errata` route・`unblind_map.json`・`build_secret.json`・`manifest.json` = 404、UI は「レビューは封印済みです。」、`events.jsonl` は browser 操作の前後で byte 同一。
- 注記: dev 機で本体 `cinemantis.exe` が稼働中だったため、実 probe は fixture 起動を正しく拒否した（下記 C）。browser fixture だけ `CM_PF0G3_FIXTURE_STUB_PROBE=1` で probe を stub にした。

## B. CSP（最終）
`default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'none'; font-src 'none'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'`
- **`unsafe-inline` を除去**。単一 HTML を `index.html` + `app.css` + `app.js` に分割（inline `<script>` / `<style>` / `style=` 属性 / event handler なし）。`/app.css` `/app.js` は同一 origin で配信（token 不要・secret を含まない・Host/Origin 検査は適用）。3 ファイルとも `ui_package_sha256` の pin 対象（init と seal の 8 pin の 1 つ）。
- 実ブラウザで実証: inline script、`data:` script、inline style 属性、cross-origin 画像・fetch が全てブロックされる。静的テスト: `innerHTML` / `eval` / `new Function` / 追加 network API なし、`fetch` は 1 箇所（相対 path）。
- 残余リスク: なし（architecture 拡張なし）。外部 origin・poster は許可していない。

## C. reviewer binary
- `services/pr4_pf0g3_reviewer.rs`（`RuntimeConfig` / `launch` / `env_presence`）+ `examples/pf0g3_reviewer.rs`（薄い main。example なので **製品 bundle には入らない**。`lib.rs` に re-export 1 行）。
- 設定は環境変数のみ（`CM_PF0G3_REVIEWER_DIR / LEDGER_DIR / CHECKPOINT_DIR / CACHE_DIR / PORT / TMDB_API_KEY`、test 専用 `TMDB_LOOPBACK`）。必須欠落・相対 path・不正 port・不正 key 形式・reviewer 側と audit 側の同一/入れ子 dir・loopback 以外の TMDB 先は全て Err（fail closed）。key は専用変数（本体の設定・DB と共有しない）で、Debug は `<redacted>`、Err・presence 報告に値も長さも出ない。package / ledger / cache / checkpoint に key が書かれないことを test で走査。
- `SystemProcessLister`: Windows は `System32\tasklist.exe` を絶対 path で実行（PATH 差し替え対策）。実機 test で一覧取得・自プロセスの検出を確認。
- 実機: 設定なし → exit 2（presence のみ表示）。設定あり＋本体 `cinemantis.exe` 稼働中 → `MainAppRunning` で exit 3、ledger / dir に触れない。一覧が取れない場合も拒否。TMDB production へ出るのは env に `TMDB_LOOPBACK` が無い通常運用のときだけで、test は loopback のみ。

## D. fail-closed traceability
| # | 条件 | 既存 test（拒否コード / 場所） |
|---|---|---|
| 1 | wrong PF0H commit anchor | `pr4_pf0i_tests::t38_required_fail_closed_matrix`（`load_pf0h_final_membership_anchored` + `0000…` → `pf0h_final_manifest_invalid`） |
| 2 | wrong final manifest SHA | `pf0i_e2e::e3_every_fail_closed_path_on_real_artifacts`（`pf0h_final_manifest_mismatch`、raw SHA を anchor にしても拒否）、`t37` |
| 3 | wrong membership commitment | `e3`（`membership_commitment_mismatch`、eligibility rule version 違いも） |
| 4 | malformed canonical JSON | `t38`（`{"a":1.5}`・非 UTF-8）、`t36_loader_is_fail_closed`（浮動小数・壊れた JSON・`authoritative=false`・別 protocol） |
| 5 | wrong build_secret_commitment | `t38`（init/seal 双方を変えても `build_secret_commitment_mismatch`）、`pf0g2::m2_wrong_build_secret_commitment_fails_closed_before_any_output`、`e3`（別 secret の bundle） |
| 6 | malformed / unexpected unblind bundle | `t38`（未知キー・空 units → `unblind_bundle_not_bijective`）、`e3`（1 byte 改変 → `unblind_bundle_sha_mismatch`） |
| 7 | pin mismatch | `pf0g2::all_eight_pins_must_match_between_init_and_seal_including_ui_and_a8`（8 pin を 1 つずつ変更 → `PinMismatch(名前)`）、`the_api_refuses_to_start_when_the_ui_package_differs_from_the_init_pin`、`e3`（`review_seal_sha_mismatch` / `machine_freeze_commitment_mismatch`） |
| 8 | missing required anchor | `t38`（5 anchor + `review_seal_sha256` 空 → `anchor_missing`）、`e4_pre_seal_ledger_is_refused`（seal 欠落）、`e3`（`seal_missing_or_duplicated`） |

追加 test なし・source 変更なし（matrix のみ）。

## E. 回帰
`cargo test --lib`: **687 passed / 0 failed / 14 ignored**（base 681 + PF0G3 の新規 6、ignored は +1 = 手動の browser fixture server）。e1–e4 / t38 / PF0G2 / PF0H1 / PF0I PASS。`git diff --check` clean。

## 残る LOW → `DEFERRED_FIXES.md`（G3-1…G3-4）
PF0G3 追加ファイル: `pr4_pf0g3_reviewer.rs` / `pr4_pf0g3_tests.rs` / `examples/pf0g3_reviewer.rs` / `pr4_pf0g2_ui/app.{css,js}` / `docs/pf0g3_evidence/*`。
