//! 照合前スナップショット（PR2）。
//!
//! 自動照合の入力は「TMDB 適用や NAS 整理で書き換わらない値」だけから作る。
//! works.title / year / media_kind などは TMDB 由来の値で上書きされるため、
//! 照合に使うと「TMDB の答えを見てから TMDB を検索する」ことになり、
//! 評価が本来より良く見えてしまう（target leakage）。
//!
//! そのため、読む列を allowlist で固定し、照合の入力は
//! [`LocalEvidence`] 型（このモジュールでしか作れない）を経由させる。

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

/// スナップショットの形式。run に記録し、後からの再現に使う。
/// PR2.5 で埋め込みメタデータ（embedded / filename_year / title_guess_year）を足したので -2。
pub const STATE_SCHEMA_VERSION: &str = "cm-prematch-2";

// ─── 読んでよい列（allowlist） ────────────────────────────────────────────────
//
// ここに無い列は照合の入力に入れない。特に works.title / original_title / year /
// release_date / runtime_sec / imdb_id / synopsis / genres_json / country_json /
// reading / country_type / media_category / media_kind は TMDB 適用で書き換わる。

const WORK_COLUMNS: &str = "w.id, w.title_guess, w.created_at";
const FILE_COLUMNS: &str = "f.original_file_name, f.original_rel_path, f.original_captured, \
                            f.renamed_by_app, f.extension, f.duration_sec, f.file_path, \
                            f.container_tags_json, f.tags_provenance, f.tags_provider_hint, \
                            f.tags_captured";
const SOURCE_COLUMNS: &str = "s.media_kind";

/// 照合に使ったタイトルの出どころ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleProvenance {
    /// scan 時に付けた推定タイトル
    TitleGuess,
    /// 元ファイル名から作り直した
    OriginalFileName,
    /// 手がかりが無い
    None,
}

/// 入力がどれだけ信用できるか（評価で live と混ぜないために記録する）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    /// scan 時の元ファイル名がそのまま残っている
    Live,
    /// 020 以前の行を現在値から後埋めした（CineMantis は改名していない）
    ReconstructedClean,
    /// CineMantis が改名した後の名前しか残っていない（評価には使えない）
    HistoricalAuditOnly,
}

impl EvidenceClass {
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceClass::Live => "live",
            EvidenceClass::ReconstructedClean => "reconstructed_clean",
            EvidenceClass::HistoricalAuditOnly => "historical_audit_only",
        }
    }
}

/// ファイル名に埋め込まれた外部 ID。PR2 では記録するだけで判断には使わない
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmbeddedId {
    /// "imdb" | "tmdb"
    pub kind: &'static str,
    pub value: String,
    /// 出どころ。今は常に filename_embedded
    pub provenance: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotFile {
    pub original_file_name: Option<String>,
    pub original_rel_path: Option<String>,
    pub original_captured: bool,
    /// CineMantis が改名した処理名（'nas_sort'）。None なら改名していない
    pub renamed_by_app: Option<String>,
    pub extension: Option<String>,
    pub duration_sec: Option<f64>,
    pub source_media_kind: String,
    /// 1 = scan 時にタグを取得、0 = 後埋め
    pub tags_captured: bool,
}

/// 埋め込みメタデータ由来の入力（PR2.5）。
/// 年は出どころを潰さずに別々に持つ。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EmbeddedInputs {
    /// 検索に使える形に整えたタグのタイトル
    pub title: Option<String>,
    /// 整形前のタグのタイトル
    pub title_raw: Option<String>,
    pub show: Option<String>,
    /// タグの年（配信年のことがある）
    pub year: Option<i32>,
    /// 作品そのものが変わりうる版の表記
    pub cut_editions: Vec<String>,
    pub audio_languages: Vec<String>,
    pub subtitle_languages: Vec<String>,
    /// 出演者（PR2.5 では判断に使わず、記録だけ）
    pub cast: Vec<String>,
    /// あらすじ（PR2.5 では判断に使わず、PR3 の初期 state にも入れない）
    pub description: Option<String>,
    pub episode_id: Option<String>,
    /// 配信元の慣習で「映画」と分かるか（provider が分かるときだけ true）
    pub episode_id_marks_movie: bool,
    /// provider_known / pipeline_known / unknown / suspicious
    pub provenance: Option<String>,
    pub provider_hint: Option<String>,
    /// 長さ上限で切り詰めた値があるか
    pub truncated: bool,
}

/// 検索語の出どころ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuerySource {
    /// ファイル名・title_guess 由来（rules-1 と同じ旧経路）
    Legacy,
    /// 埋め込みメタデータ由来
    Embedded,
}

impl QuerySource {
    pub fn as_str(self) -> &'static str {
        match self {
            QuerySource::Legacy => "legacy",
            QuerySource::Embedded => "embedded",
        }
    }
}

/// 1回分の検索指示
#[derive(Debug, Clone, PartialEq)]
pub struct SearchInput {
    pub source: QuerySource,
    pub title: String,
    /// 明示した年。None なら検索語から解析した年を使う（旧経路と同じ）
    pub year: Option<i32>,
}

/// 照合前スナップショット。run の input_snapshot_json に丸ごと保存する
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreMatchSnapshot {
    pub state_schema_version: &'static str,
    pub work_id: i64,
    pub work_created_at: Option<String>,
    pub title_guess: Option<String>,
    /// 照合に使うタイトル
    pub derived_title: String,
    pub title_provenance: TitleProvenance,
    /// "movie" | "tv" | "unknown"（ソースの設定。works.media_kind は使わない）
    pub media_kind: String,
    /// パスから推定した洋邦（works.country_type は使わない）
    pub country_type: String,
    /// 元ファイル名から取れた年
    pub filename_year: Option<i32>,
    /// title_guess から取れた年
    pub title_guess_year: Option<i32>,
    /// 埋め込みメタデータ（PR2.5）
    pub embedded: EmbeddedInputs,
    pub files: Vec<SnapshotFile>,
    pub embedded_ids: Vec<EmbeddedId>,
    pub evidence_class: EvidenceClass,
}

/// 照合の入力。[`PreMatchSnapshot`] から作るか、ユーザーが明示した検索語からしか作れない
/// （フィールドが private なので他のモジュールからは組み立てられない）。
#[derive(Debug, Clone, PartialEq)]
pub struct LocalEvidence {
    /// 旧経路（rules-1 / rules-safe）の検索語。ファイル名・title_guess 由来
    title: String,
    /// 埋め込みメタデータ由来の検索語（PR2.5）
    embedded_title: Option<String>,
    media_kind: String,
    filename_year: Option<i32>,
    embedded_year: Option<i32>,
    user_override: bool,
}

impl LocalEvidence {
    /// 旧経路の検索語（rules-1 の入力はこれだけ）
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn embedded_title(&self) -> Option<&str> {
        self.embedded_title.as_deref()
    }

    pub fn media_kind(&self) -> &str {
        &self.media_kind
    }

    pub fn embedded_year(&self) -> Option<i32> {
        self.embedded_year
    }

    pub fn filename_year(&self) -> Option<i32> {
        self.filename_year
    }

    /// 投げる検索の一覧。旧経路の検索語は必ず先頭に入る。
    /// 埋め込み由来が同じ（正規化して一致し、年も同じ）なら1回しか投げない。
    pub fn search_inputs(&self) -> Vec<SearchInput> {
        let mut inputs = vec![SearchInput {
            source: QuerySource::Legacy,
            title: self.title.clone(),
            year: None,
        }];
        if let Some(embedded) = self.embedded_title.as_deref() {
            let same_title = normalize_for_compare(embedded) == normalize_for_compare(&self.title);
            let adds_year = self.embedded_year.is_some() && self.embedded_year != self.filename_year;
            if !same_title || adds_year {
                inputs.push(SearchInput {
                    source: QuerySource::Embedded,
                    title: embedded.to_string(),
                    year: self.embedded_year,
                });
            }
        }
        inputs
    }

    /// ユーザーが検索語や種別を指定したか（true の run は自動照合の評価から外す）。
    /// PR2 では手動検索が run を作らないので、記録用の印として持つだけ
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_user_override(&self) -> bool {
        self.user_override
    }

    /// 候補ダイアログの手動検索。ユーザーの指定だけを上書きする
    pub fn with_user_override(
        &self,
        query: Option<&str>,
        media_type_hint: Option<&str>,
    ) -> LocalEvidence {
        let mut next = self.clone();
        if let Some(query) = query.map(str::trim).filter(|q| !q.is_empty()) {
            next.title = query.to_string();
            // ユーザーが打った語を優先し、タグ由来の検索語は足さない
            next.embedded_title = None;
            next.user_override = true;
        }
        if let Some(hint) = media_type_hint.filter(|h| *h == "movie" || *h == "tv") {
            next.media_kind = hint.to_string();
            next.user_override = true;
        }
        next
    }
}

impl PreMatchSnapshot {
    /// 照合前入力から検索の入力を作る（本番の自動照合はこれだけを使う）
    pub fn local_evidence(&self) -> LocalEvidence {
        LocalEvidence {
            title: self.derived_title.clone(),
            embedded_title: self.embedded.title.clone(),
            media_kind: self.media_kind.clone(),
            filename_year: self.filename_year.or(self.title_guess_year),
            embedded_year: self.embedded.year,
            user_override: false,
        }
    }

    /// rules-safe / rules-tags-shadow に渡す、埋め込みメタデータ由来の材料
    pub fn embedded_evidence(&self) -> crate::services::metadata_matcher::EmbeddedEvidence {
        crate::services::metadata_matcher::EmbeddedEvidence {
            embedded_year: self.embedded.year,
            filename_year: self.filename_year.or(self.title_guess_year),
            audio_languages: self.embedded.audio_languages.clone(),
            cut_editions: self.embedded.cut_editions.clone(),
            episode_id_marks_movie: self.embedded.episode_id_marks_movie,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// 1作品の照合前入力を読む。allowlist の列しか読まない。
    pub fn capture(conn: &Connection, work_id: i64) -> Result<PreMatchSnapshot, String> {
        let work_sql = format!("SELECT {WORK_COLUMNS} FROM works w WHERE w.id = ?1");
        let (id, title_guess, work_created_at) = conn
            .query_row(&work_sql, rusqlite::params![work_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("work {work_id} not found"))?;

        let files_sql = format!(
            "SELECT {FILE_COLUMNS}, {SOURCE_COLUMNS}
             FROM work_parts wp
             JOIN files f ON f.id = wp.file_id
             JOIN sources s ON s.id = f.source_id
             WHERE wp.work_id = ?1
             ORDER BY wp.part_no, f.id"
        );
        let mut stmt = conn.prepare(&files_sql).map_err(|e| e.to_string())?;
        let rows: Vec<(SnapshotFile, String, Option<TagRow>)> = stmt
            .query_map(rusqlite::params![id], |row| {
                let tags_json: Option<String> = row.get(7)?;
                Ok((
                    SnapshotFile {
                        original_file_name: row.get(0)?,
                        original_rel_path: row.get(1)?,
                        original_captured: row.get::<_, Option<i64>>(2)?.unwrap_or(0) == 1,
                        renamed_by_app: row.get(3)?,
                        extension: row.get(4)?,
                        duration_sec: row.get(5)?,
                        source_media_kind: row.get::<_, Option<String>>(11)?
                            .unwrap_or_else(|| "unknown".to_string()),
                        tags_captured: row.get::<_, Option<i64>>(10)?.unwrap_or(0) == 1,
                    },
                    // 洋邦の推定にだけ使う。改名前の相対パスを優先する
                    row.get::<_, Option<String>>(1)?
                        .unwrap_or_else(|| row.get::<_, String>(6).unwrap_or_default()),
                    tags_json.map(|json| TagRow {
                        json,
                        provenance: row.get::<_, Option<String>>(8).unwrap_or(None),
                        provider_hint: row.get::<_, Option<String>>(9).unwrap_or(None),
                        file_path: row.get::<_, Option<String>>(6).unwrap_or(None),
                    }),
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        let mut files = Vec::with_capacity(rows.len());
        let mut paths = Vec::with_capacity(rows.len());
        let mut tag_rows = Vec::new();
        for (file, path, tags) in rows {
            files.push(file);
            paths.push(path);
            if let Some(tags) = tags {
                tag_rows.push(tags);
            }
        }
        // 複数ファイルの作品では、中身のあるタグを持つ最初のファイルを使う
        let embedded = tag_rows
            .iter()
            .filter_map(|row| row.to_inputs())
            .find(|inputs| inputs.title.is_some() || inputs.year.is_some())
            .or_else(|| tag_rows.first().and_then(|row| row.to_inputs()))
            .unwrap_or_default();

        let title_guess = title_guess.filter(|t| !t.trim().is_empty());
        let first_original = files
            .iter()
            .find_map(|f| f.original_file_name.as_deref().filter(|n| !n.trim().is_empty()));
        let (derived_title, title_provenance) = match (&title_guess, first_original) {
            (Some(guess), _) => (guess.clone(), TitleProvenance::TitleGuess),
            (None, Some(name)) => (
                crate::commands::scan::estimate_title(name),
                TitleProvenance::OriginalFileName,
            ),
            (None, None) => (String::new(), TitleProvenance::None),
        };

        let media_kind = files
            .iter()
            .map(|f| f.source_media_kind.as_str())
            .find(|kind| *kind == "movie" || *kind == "tv")
            .unwrap_or("unknown")
            .to_string();

        let country_type = paths
            .iter()
            .map(|path| infer_country_type(path))
            .find(|country| *country != "unknown")
            .unwrap_or("unknown")
            .to_string();

        let mut embedded_ids = Vec::new();
        for name in files
            .iter()
            .filter_map(|f| f.original_file_name.as_deref())
            .chain(paths.iter().map(String::as_str))
        {
            for found in embedded_ids_in(name) {
                if !embedded_ids.contains(&found) {
                    embedded_ids.push(found);
                }
            }
        }

        let evidence_class = evidence_class_for(&files);
        let filename_year = files
            .iter()
            .filter_map(|f| f.original_file_name.as_deref())
            .find_map(crate::services::container_tags::extract_year);
        let title_guess_year = title_guess
            .as_deref()
            .and_then(crate::services::container_tags::extract_year);

        Ok(PreMatchSnapshot {
            state_schema_version: STATE_SCHEMA_VERSION,
            work_id: id,
            work_created_at,
            title_guess,
            derived_title,
            title_provenance,
            media_kind,
            country_type,
            filename_year,
            title_guess_year,
            embedded,
            files,
            embedded_ids,
            evidence_class,
        })
    }
}

/// files から読んだタグ1行分
struct TagRow {
    json: String,
    provenance: Option<String>,
    provider_hint: Option<String>,
    file_path: Option<String>,
}

impl TagRow {
    fn to_inputs(&self) -> Option<EmbeddedInputs> {
        use crate::services::container_tags::ContainerTags;
        let tags = ContainerTags::from_json(&self.json)?;
        let path_hint = self.file_path.as_deref();
        Some(EmbeddedInputs {
            title: tags.cleaned_title(),
            title_raw: tags.title.clone(),
            show: tags.cleaned_show(),
            year: tags.year(),
            cut_editions: tags.cut_editions(),
            audio_languages: tags.audio_languages.clone(),
            subtitle_languages: tags.subtitle_languages.clone(),
            cast: tags.cast(),
            description: tags.description.clone(),
            episode_id: tags.episode_id.clone(),
            episode_id_marks_movie: tags.episode_id_marks_movie(path_hint),
            provenance: self
                .provenance
                .clone()
                .or_else(|| Some(tags.provenance(path_hint).as_str().to_string())),
            provider_hint: self
                .provider_hint
                .clone()
                .or_else(|| tags.provider_hint().map(str::to_string)),
            truncated: !tags.truncated.is_empty(),
        })
    }
}

/// 検索語が実質同じかを見るための正規化（記号と空白を落とす）
fn normalize_for_compare(value: &str) -> String {
    crate::services::container_tags::normalize_widths(value)
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn evidence_class_for(files: &[SnapshotFile]) -> EvidenceClass {
    if files.is_empty() || files.iter().any(|f| f.original_file_name.is_none()) {
        return EvidenceClass::HistoricalAuditOnly;
    }
    if files.iter().any(|f| f.renamed_by_app.is_some()) {
        return EvidenceClass::HistoricalAuditOnly;
    }
    if files.iter().all(|f| f.original_captured) {
        EvidenceClass::Live
    } else {
        EvidenceClass::ReconstructedClean
    }
}

/// パスに含まれるフォルダ名から洋邦を推定する（scan と共通）
pub fn infer_country_type(path: &str) -> &'static str {
    let haystack = path.to_lowercase();
    const DOMESTIC_MARKERS: [&str; 9] = [
        "邦画", "国内", "日本映画", "日本", "japanese", "japan", "jp-movie", "jp_movie", "domestic",
    ];
    const FOREIGN_MARKERS: [&str; 12] = [
        "洋画", "海外", "外国映画", "外国", "foreign", "western", "world cinema", "world_cinema",
        "korean", "chinese", "europe", "america",
    ];
    if DOMESTIC_MARKERS.iter().any(|m| haystack.contains(m)) {
        return "domestic";
    }
    if FOREIGN_MARKERS.iter().any(|m| haystack.contains(m)) {
        return "foreign";
    }
    "unknown"
}

/// ファイル名に埋め込まれた IMDb / TMDB の ID を抜き出す
fn embedded_ids_in(name: &str) -> Vec<EmbeddedId> {
    use regex::Regex;
    use std::sync::OnceLock;
    static IMDB: OnceLock<Regex> = OnceLock::new();
    static TMDB: OnceLock<Regex> = OnceLock::new();
    let imdb = IMDB.get_or_init(|| Regex::new(r"(?i)\btt(\d{6,9})\b").unwrap());
    let tmdb = TMDB.get_or_init(|| Regex::new(r"(?i)tmdb(?:id)?[-_=:\s]?(\d{1,9})").unwrap());

    let mut found = Vec::new();
    for caps in imdb.captures_iter(name) {
        found.push(EmbeddedId {
            kind: "imdb",
            value: format!("tt{}", &caps[1]),
            provenance: "filename_embedded",
        });
    }
    for caps in tmdb.captures_iter(name) {
        found.push(EmbeddedId {
            kind: "tmdb",
            value: caps[1].to_string(),
            provenance: "filename_embedded",
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::scan::{insert_scanned_file, insert_scanned_work, ScannedFile};
    use crate::db::test_support::*;

    fn add_file(conn: &Connection, work_id: i64, source_id: i64, root: &str, path: &str, name: &str) -> i64 {
        let file_id = insert_scanned_file(
            conn,
            &ScannedFile {
                source_id,
                root_path: root,
                file_path: path,
                file_name: name,
                extension: "mkv",
                file_size: None,
                mtime: None,
                duration_sec: Some(7200.0),
                width: None,
                height: None,
                video_codec: None,
                audio_codec: None,
                container: "mkv",
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id, part_no) VALUES (?1, ?2,
                (SELECT COALESCE(MAX(part_no), 0) + 1 FROM work_parts WHERE work_id = ?1))",
            rusqlite::params![work_id, file_id],
        )
        .unwrap();
        file_id
    }

    /// scan 直後と同じ状態の作品を作る（ソースはファイルの置き場ごとに1つ）
    fn scanned(conn: &Connection, title: &str, path: &str, name: &str) -> i64 {
        let root = path.rsplit_once('\\').map(|(dir, _)| dir).unwrap_or(path);
        let source_id = conn
            .query_row(
                "SELECT id FROM sources WHERE root_path = ?1",
                rusqlite::params![root],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or_else(|_| insert_source(conn, root));
        let work_id = insert_scanned_work(conn, "movie", "unknown", title, "unknown").unwrap();
        add_file(conn, work_id, source_id, root, path, name);
        work_id
    }

    #[test]
    fn snapshot_ignores_columns_overwritten_by_tmdb() {
        let conn = open_migrated();
        let work_id = scanned(&conn, "Alien 1979", r"D:\Import\Alien.1979.1080p.mkv", "Alien.1979.1080p.mkv");
        let before = PreMatchSnapshot::capture(&conn, work_id).unwrap();

        // TMDB 適用後の状態を作る（title / year / media_kind / country_type などが書き換わる）
        conn.execute(
            "UPDATE works SET title = 'エイリアン', original_title = 'Alien', year = 1979,
                    release_date = '1979-05-25', media_kind = 'tv', country_type = 'foreign',
                    reading = 'えいりあん', synopsis = 'テキスト', tmdb_id = 348
             WHERE id = ?1",
            rusqlite::params![work_id],
        )
        .unwrap();

        let after = PreMatchSnapshot::capture(&conn, work_id).unwrap();
        assert_eq!(before, after, "TMDB 由来の値で入力が変わらないこと");
        assert_eq!(after.derived_title, "Alien 1979");
        assert_eq!(after.media_kind, "unknown");
    }

    /// allowlist に無い列名が SQL に現れないこと（列を足すときの歯止め）
    #[test]
    fn query_columns_stay_inside_the_allowlist() {
        let sql = format!("{WORK_COLUMNS} {FILE_COLUMNS} {SOURCE_COLUMNS}");
        for forbidden in [
            "w.title,", "w.title ", "w.year", "w.release_date", "w.original_title", "w.synopsis",
            "w.genres_json", "w.country_json", "w.reading", "w.country_type", "w.media_category",
            "w.media_kind", "w.runtime_sec", "w.imdb_id", "w.tmdb_id",
        ] {
            assert!(!sql.contains(forbidden), "{forbidden} を読んではいけない");
        }
    }

    #[test]
    fn title_falls_back_to_the_original_file_name() {
        let conn = open_migrated();
        let work_id = scanned(&conn, "Alien 1979", r"D:\Import\Alien.1979.1080p.mkv", "Alien.1979.1080p.mkv");
        conn.execute("UPDATE works SET title_guess = NULL WHERE id = ?1", rusqlite::params![work_id])
            .unwrap();

        let snapshot = PreMatchSnapshot::capture(&conn, work_id).unwrap();
        assert_eq!(snapshot.derived_title, "Alien 1979");
        assert_eq!(snapshot.title_provenance, TitleProvenance::OriginalFileName);
        assert_eq!(snapshot.local_evidence().title(), "Alien 1979");
    }

    #[test]
    fn evidence_class_covers_captured_backfilled_and_renamed() {
        let conn = open_migrated();
        let live = scanned(&conn, "A", r"D:\Import\A.2001.mkv", "A.2001.mkv");
        assert_eq!(
            PreMatchSnapshot::capture(&conn, live).unwrap().evidence_class,
            EvidenceClass::Live
        );

        // 020 以前の行（元ファイル名を現在値から後埋めした）
        let backfilled = insert_work(&conn, "B");
        let source_id = insert_source(&conn, r"D:\Import2");
        conn.execute(
            "INSERT INTO files (source_id, file_path, file_name) VALUES (?1, ?2, ?3)",
            rusqlite::params![source_id, r"D:\Import2\B.2002.mkv", "B.2002.mkv"],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO work_parts (work_id, file_id) VALUES (?1, ?2)",
            rusqlite::params![backfilled, file_id],
        )
        .unwrap();
        crate::db::backfill_prematch_inputs(&conn).unwrap();
        assert_eq!(
            PreMatchSnapshot::capture(&conn, backfilled).unwrap().evidence_class,
            EvidenceClass::ReconstructedClean
        );

        // CineMantis が改名した
        let renamed = scanned(&conn, "C", r"D:\Import3\C.2003.mkv", "C.2003.mkv");
        conn.execute(
            "UPDATE files SET renamed_by_app = 'nas_sort'
             WHERE id IN (SELECT file_id FROM work_parts WHERE work_id = ?1)",
            rusqlite::params![renamed],
        )
        .unwrap();
        assert_eq!(
            PreMatchSnapshot::capture(&conn, renamed).unwrap().evidence_class,
            EvidenceClass::HistoricalAuditOnly
        );

        // ファイルが無い作品も評価には使えない
        let no_file = insert_work(&conn, "D");
        assert_eq!(
            PreMatchSnapshot::capture(&conn, no_file).unwrap().evidence_class,
            EvidenceClass::HistoricalAuditOnly
        );
    }

    #[test]
    fn country_type_comes_from_the_path() {
        assert_eq!(infer_country_type(r"【01】洋画/【あ-】/【あ】/x.mkv"), "foreign");
        assert_eq!(infer_country_type(r"【02】邦画/【さ-】/【さ】/x.mkv"), "domestic");
        assert_eq!(infer_country_type("Import/new/x.mkv"), "unknown");
    }

    #[test]
    fn embedded_ids_are_recorded_but_not_used_for_matching() {
        let conn = open_migrated();
        let work_id = scanned(
            &conn,
            "Alien",
            r"D:\Import\Alien (1979) [tmdbid-348] [tt0078748].mkv",
            "Alien (1979) [tmdbid-348] [tt0078748].mkv",
        );
        let snapshot = PreMatchSnapshot::capture(&conn, work_id).unwrap();
        assert!(snapshot.embedded_ids.contains(&EmbeddedId {
            kind: "imdb",
            value: "tt0078748".into(),
            provenance: "filename_embedded",
        }));
        assert!(snapshot.embedded_ids.contains(&EmbeddedId {
            kind: "tmdb",
            value: "348".into(),
            provenance: "filename_embedded",
        }));
        // 検索語には混ぜない
        assert!(!snapshot.local_evidence().title().contains("tmdbid"));
    }

    // ─── PR2.5: 埋め込みメタデータ ───────────────────────────────────────────

    fn tag_file(conn: &Connection, work_id: i64, pairs: &[(&str, &str)], streams: &[(&str, &str)]) {
        use crate::services::container_tags::ContainerTags;
        let map: std::collections::BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let streams: Vec<(String, Option<String>)> = streams
            .iter()
            .map(|(kind, lang)| (kind.to_string(), Some(lang.to_string())))
            .collect();
        let tags = ContainerTags::from_ffprobe(&map, &streams);
        let file_id: i64 = conn
            .query_row(
                "SELECT file_id FROM work_parts WHERE work_id = ?1",
                rusqlite::params![work_id],
                |row| row.get(0),
            )
            .unwrap();
        crate::commands::scan::store_container_tags(conn, file_id, &tags, None, true).unwrap();
    }

    #[test]
    fn embedded_inputs_keep_each_year_source_separate() {
        let conn = open_migrated();
        let work_id = scanned(&conn, "The Guilty", r"D:\Import\THE GUILTY.mkv", "THE GUILTY.mkv");
        tag_file(
            &conn,
            work_id,
            &[
                ("title", "THE GUILTY／ギルティ(字幕版)"),
                ("date", "2019"),
                ("artist", "ヤコブ・セーダーグレン, イェシカ・ディナウエ"),
                ("encoder", "Lavf58.76.100"),
            ],
            &[("audio", "dan"), ("subtitle", "jpn")],
        );

        let snapshot = PreMatchSnapshot::capture(&conn, work_id).unwrap();
        assert_eq!(snapshot.embedded.title.as_deref(), Some("THE GUILTY/ギルティ"));
        assert_eq!(snapshot.embedded.title_raw.as_deref(), Some("THE GUILTY／ギルティ(字幕版)"));
        assert_eq!(snapshot.embedded.year, Some(2019));
        assert_eq!(snapshot.embedded.audio_languages, vec!["dan".to_string()]);
        assert_eq!(snapshot.embedded.cast.len(), 2);
        assert_eq!(snapshot.embedded.provenance.as_deref(), Some("pipeline_known"));
        // 年の出どころは潰さない
        assert_eq!(snapshot.filename_year, None);
        assert_eq!(snapshot.title_guess_year, None);

        // rules-safe / shadow に渡す材料
        let evidence = snapshot.embedded_evidence();
        assert_eq!(evidence.embedded_year, Some(2019));
        assert_eq!(evidence.filename_year, None);
        assert!(!evidence.is_empty());
    }

    #[test]
    fn filename_year_is_read_from_the_original_name() {
        let conn = open_migrated();
        let work_id = scanned(&conn, "Alien 1979", r"D:\Import\Alien.1979.mkv", "Alien.1979.mkv");
        let snapshot = PreMatchSnapshot::capture(&conn, work_id).unwrap();
        assert_eq!(snapshot.filename_year, Some(1979));
        assert_eq!(snapshot.title_guess_year, Some(1979));
        assert_eq!(snapshot.embedded.year, None);
    }

    #[test]
    fn search_inputs_add_the_embedded_query_only_when_it_differs() {
        let conn = open_migrated();
        let work_id = scanned(&conn, "THE GUILTY", r"D:\Import\THE GUILTY.mkv", "THE GUILTY.mkv");

        // 同じタイトル・年も無し → 検索は旧経路の1回だけ
        tag_file(&conn, work_id, &[("title", "THE GUILTY")], &[]);
        let inputs = PreMatchSnapshot::capture(&conn, work_id)
            .unwrap()
            .local_evidence()
            .search_inputs();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].source, QuerySource::Legacy);

        // 年が足される → タグ側でもう1回引く
        let with_year = scanned(&conn, "THE GUILTY", r"D:\Import2\THE GUILTY.mkv", "THE GUILTY.mkv");
        tag_file(&conn, with_year, &[("title", "THE GUILTY"), ("date", "2019")], &[]);
        let inputs = PreMatchSnapshot::capture(&conn, with_year)
            .unwrap()
            .local_evidence()
            .search_inputs();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[1].source, QuerySource::Embedded);
        assert_eq!(inputs[1].year, Some(2019));

        // タイトルが違えば当然2回
        let other = scanned(&conn, "告発のとき", r"D:\Import3\告発のとき.mkv", "告発のとき.mkv");
        tag_file(&conn, other, &[("title", "In The Valley Of Elah (字幕版)")], &[]);
        let evidence = PreMatchSnapshot::capture(&conn, other).unwrap().local_evidence();
        let inputs = evidence.search_inputs();
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].title, "告発のとき");
        assert_eq!(inputs[1].title, "In The Valley Of Elah");

        // ユーザーが検索語を打ったらタグ由来は足さない
        let overridden = evidence.with_user_override(Some("Elah"), None);
        assert_eq!(overridden.search_inputs().len(), 1);
        assert_eq!(overridden.search_inputs()[0].title, "Elah");
    }

    #[test]
    fn user_override_is_marked_and_limited_to_what_was_given() {
        let conn = open_migrated();
        let work_id = scanned(&conn, "Alien 1979", r"D:\Import\Alien.1979.mkv", "Alien.1979.mkv");
        let evidence = PreMatchSnapshot::capture(&conn, work_id).unwrap().local_evidence();
        assert!(!evidence.is_user_override());

        let overridden = evidence.with_user_override(Some(" エイリアン "), Some("tv"));
        assert_eq!(overridden.title(), "エイリアン");
        assert_eq!(overridden.media_kind(), "tv");
        assert!(overridden.is_user_override());

        // 空文字や不正な種別は無視する
        let untouched = evidence.with_user_override(Some("   "), Some("anime"));
        assert_eq!(untouched.title(), "Alien 1979");
        assert_eq!(untouched.media_kind(), "unknown");
        assert!(!untouched.is_user_override());
    }
}
