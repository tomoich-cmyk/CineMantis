# PR4-PF0E-0: matcher input inventory（read-only audit・未 commit）

目的: H2 登録時に「後で同じ評価入力だった」と証明するために、何を hash すべきかの棚卸し。ledger 実装・T0・登録はしていない。
調査対象: `prematch_snapshot.rs`（入力の唯一の入口 `PreMatchSnapshot::capture` → `LocalEvidence` / `EmbeddedEvidence`）、`commands/tmdb.rs`（`fetch_candidates` / `match_once`）、`audit_run.rs`、`tmdb_client.rs`、`db.rs`、`commands/scan.rs`。

## 結論（先に）
1. production の matcher 入力は既に allowlist 化されている（`WORK_COLUMNS` / `FILE_COLUMNS` / `SOURCE_COLUMNS`、形式 `cm-prematch-2`）。評価の入力はこの `PreMatchSnapshot` 経由のみで、works.title / year などの TMDB 由来列は読まない。
2. 判定に実際に効くのは、`LocalEvidence`（derived_title・embedded_title・media_kind・filename_year(or title_guess_year)・embedded_year）と `EmbeddedEvidence`（embedded_year・audio_languages・cut_editions・episode_id_marks_movie）だけ。
3. **raw 列は enrollment 後に変わりうる**（下記）。よって「後で live DB から再生成して hash が一致するか」で入力同一性を証明する設計は成立しない。**登録時点の canonical payload を外部に保存し、その hash を ledger に入れる**必要がある（payload 自体は性能分析に使わない）。
4. 推奨 hash 対象 = 「raw 列の最小十分集合」＋「raw だけからは再生成できない値」＋ code/version の固定。derived 値は raw + version で再生成できるものは hash から外す。

## 変更の可能性（enrollment 後）
| 列 | 保護 | 変わる経路 |
|---|---|---|
| files.original_file_name / original_rel_path / original_captured | **DB trigger で不変**（`trg_files_original_immutable`） | 変わらない（NULL からの初回設定のみ） |
| works.title_guess | 保護なし。製品コードに更新は無い（テストのみ） | scan 時に設定。schema 上は書き換え可能 |
| files.container_tags_json / tags_* | 保護なし | `store_container_tags`（再 scan・後埋め）で上書きされうる（tags_captured=0 の後埋めあり） |
| files.file_path | 保護なし | NAS 整理・TMDB 適用の改名で変わる（`renamed_by_app`） |
| files.renamed_by_app | 保護なし | 改名で 'nas_sort' が付く → evidence_class が live から落ちる |
| sources.media_kind | ユーザー設定 | 設定変更で変わる |
| works.created_at | 保護なし（PF0D で確認済み） | enrollment 時刻には使わない |

## 項目表
凡例: cand=候補生成に影響 / dec=判定に影響 / det=他の凍結値から決定論的 / hash=hash 対象

### A. direct matcher inputs（判定・検索に直接入る）
| field / source | where read | how used | cand | dec | det | hash | reason |
|---|---|---|---|---|---|---|---|
| works.title_guess | `capture` WORK_COLUMNS | derived_title の第一候補、title_guess_year | yes | yes | no | **yes** | 保護なし・raw |
| files.original_file_name（part_no,id 順） | `capture` | title_guess 無しのとき derived_title（`estimate_title`）、filename_year、embedded_ids、evidence_class | yes | yes | no | **yes** | trigger で不変だが raw の根拠 |
| files.original_rel_path | `capture` | 洋邦推定（record only）、`tags path_hint` の元 | no | no* | no | yes（path_hint 用） | *path_hint が episode_id_marks_movie に効く |
| files.original_captured / renamed_by_app | `capture` | evidence_class（cohort） | no | no | no | **yes** | cohort 所属の根拠。登録時の値を固定 |
| files.container_tags_json | `capture`→`TagRow::to_inputs` | embedded.title/year/audio/cut_editions/episode_id_marks_movie | yes | yes | no | **yes** | 保護なし・上書きされうる |
| files.tags_provenance / tags_provider_hint | `capture` | episode_id_marks_movie の provider 判定、provenance | no | yes（間接） | no | **yes** | NULL のとき tags と path_hint から再計算される |
| files.file_path | `capture`（path_hint・洋邦の fallback） | episode_id_marks_movie、provenance fallback | no | yes（間接） | no | **要判断** | 改名で変わる。下記 Q1 |
| sources.media_kind | `capture` SOURCE_COLUMNS | media_kind → 検索の movie/tv 分岐、rules | yes | yes | no | **yes** | ユーザー設定で変わりうる |
| work_parts.part_no（並び順）/ files.id | `capture` ORDER BY | 複数ファイルの先頭選択 | yes | yes | no | yes（順序として） | 並びが入力選択を決める |

### B. derived inputs（raw + code で決定論的）
| field | 生成元 | cand | dec | det | hash | reason |
|---|---|---|---|---|---|---|
| derived_title / title_provenance | title_guess or `estimate_title(original_file_name)` | yes | yes | yes | no（raw+version） | 二重保持を避ける。ただし payload には残して検算に使う案あり |
| filename_year / title_guess_year | `container_tags::extract_year` | yes | yes | yes | no | 同上 |
| embedded.title / title_raw / year / cut_editions / audio_languages / episode_id_marks_movie | `ContainerTags` の関数 | yes | yes | yes（tags_json+path_hint+version） | no | 同上。ただし path_hint 依存あり |
| media_kind（works 側の導出） | 最初の movie/tv の source | yes | yes | yes | no | sources.media_kind から |
| country_type / embedded_ids / evidence_class | path 推定・regex・files | no | no（record only / cohort） | yes | evidence_class のみ登録時に固定 | cohort 所属の判定に使う |
| search_inputs / query_plan | `LocalEvidence::search_inputs`・`rules_v1::search_plan_v1` | yes | yes | yes | no | code version で決まる |

### C. candidate-generation（TMDB 検索・候補生成）
| item | where | 備考 |
|---|---|---|
| クエリ文字列・年・retry_without_year・movie/tv 分岐 | `query_plan_for` / `rules_v1::search_plan_v1` | A/B の入力 + code で決まる。**hash 対象外**（次 gate の machine-output 凍結で固定） |
| TMDB request パラメータ | `tmdb_client.rs`: `language=ja-JP`, `include_adult=false`、ページ指定なし | code 定数。code version で固定 |
| 候補一覧・スコア・順位 | `score_movie/tv`, `merge_and_rank` | TMDB 応答に依存。**enrollment の hash に入れない** |
| TMDB 応答そのもの | 外部 | 再現不能 → 後段で凍結 |

### D. runtime / environment dependencies（hash でなく version/commit として固定）
| item | 値・場所 |
|---|---|
| matcher policy | `POLICY_VERSION = rules-safe-2`、`THRESHOLD_AUTO 75 / CANDIDATE 40`、`YEAR_TOLERANCE 1`、`EMBEDDED_YEAR_TOLERANCE 1` |
| rules-1（凍結実装） | `rules_v1.rs`（`THRESHOLD_AUTO_V1 75`） |
| parser | `title_parser`、`estimate_title`、`container_tags`（`TAGS_SCHEMA_VERSION 1`）、スナップショット形式 `cm-prematch-2` |
| TMDB 呼び出し | `ja-JP`、`include_adult=false`、API base |
| コード版 | enrollment tool の commit / sha（`enrollment_tool_sha256`）と、machine-output 生成時の commit |
| locale・時刻 | 判定に使われる箇所は未確認（`strftime now` は記録用の列のみ）。要追加確認ではなく、生成物の凍結時に実測で確認 |

### E. 除外可能（表示専用・監査専用・判定非影響）
`works.title / original_title / year / release_date / runtime / imdb_id / synopsis / genres / country / reading / country_type / media_category / media_kind`（TMDB 由来で、allowlist に入っていない）、`files.duration_sec`・`extension`（`SnapshotFile` に入るが LocalEvidence に出ない）、embedded.cast / description / subtitle_languages / show / truncated（record only）、`work_created_at`、Jev shadow（production 判定に使われず結果は捨てる）。

### F. 再現不能 / 外部依存（入力 hash では固定できない）
TMDB の検索応答（時間で変わる）、TMDB 側のデータ更新、ネットワーク失敗時の ERROR run、`strftime('now')` 系の時刻列。→ 次 gate「machine-output generation の凍結」で、候補・応答を成果物として保存・hash する。

## 判断してほしい点
- **Q1（path_hint）**: `episode_id_marks_movie` と provenance の fallback は `files.file_path`（改名で変わる）を使う。登録時に `original_rel_path` ベースで固定するか、現在の `file_path` で `episode_id_marks_movie` / provenance を**値として** payload に入れるか。推奨: 後者（derived 値 episode_id_marks_movie・provenance・provider_hint だけは登録時の値を payload に持つ。file_path 自体は payload に入れない＝絶対パスを残さない）。
- **Q2（payload の形）**: 推奨は「raw 最小集合（A の hash=yes 項目）＋ 上の 3 derived 値」の canonical JSON を外部に保存し、sha256 を ledger に入れる。derived_title など raw+version で出る値は入れない。代案は `PreMatchSnapshot` 全体（cm-prematch-2）を保存。後者は二重保持と正規化差のリスクがあるが、再生成なしで入力を丸ごと固定できる。
- **Q3（再生成の検算）**: 登録時に payload から `LocalEvidence` を再生成した結果の別 hash（evidence_sha256）を併記し、version が変わった後も「同じ検索語・同じ判定入力か」を検算できるようにするか。
- **Q4（cohort 所属の固定）**: evidence_class（live）は登録時の値を ledger に持つ。登録後に改名で `historical_audit_only` になっても membership は外さない（PF0D 契約どおり）。評価入力は登録時 payload を使う。
