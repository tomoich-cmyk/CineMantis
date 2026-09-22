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

/// スナップショットの形式。run に記録し、後からの再現に使う
pub const STATE_SCHEMA_VERSION: &str = "cm-prematch-1";

// ─── 読んでよい列（allowlist） ────────────────────────────────────────────────
//
// ここに無い列は照合の入力に入れない。特に works.title / original_title / year /
// release_date / runtime_sec / imdb_id / synopsis / genres_json / country_json /
// reading / country_type / media_category / media_kind は TMDB 適用で書き換わる。

const WORK_COLUMNS: &str = "w.id, w.title_guess, w.created_at";
const FILE_COLUMNS: &str = "f.original_file_name, f.original_rel_path, f.original_captured, \
                            f.renamed_by_app, f.extension, f.duration_sec, f.file_path";
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
    pub files: Vec<SnapshotFile>,
    pub embedded_ids: Vec<EmbeddedId>,
    pub evidence_class: EvidenceClass,
}

/// 照合の入力。[`PreMatchSnapshot`] から作るか、ユーザーが明示した検索語からしか作れない
/// （フィールドが private なので他のモジュールからは組み立てられない）。
#[derive(Debug, Clone, PartialEq)]
pub struct LocalEvidence {
    title: String,
    media_kind: String,
    user_override: bool,
}

impl LocalEvidence {
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn media_kind(&self) -> &str {
        &self.media_kind
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
            media_kind: self.media_kind.clone(),
            user_override: false,
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
        let rows: Vec<(SnapshotFile, String)> = stmt
            .query_map(rusqlite::params![id], |row| {
                Ok((
                    SnapshotFile {
                        original_file_name: row.get(0)?,
                        original_rel_path: row.get(1)?,
                        original_captured: row.get::<_, Option<i64>>(2)?.unwrap_or(0) == 1,
                        renamed_by_app: row.get(3)?,
                        extension: row.get(4)?,
                        duration_sec: row.get(5)?,
                        source_media_kind: row.get::<_, Option<String>>(7)?
                            .unwrap_or_else(|| "unknown".to_string()),
                    },
                    // 洋邦の推定にだけ使う。改名前の相対パスを優先する
                    row.get::<_, Option<String>>(1)?
                        .unwrap_or_else(|| row.get::<_, String>(6).unwrap_or_default()),
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        let (files, paths): (Vec<SnapshotFile>, Vec<String>) = rows.into_iter().unzip();

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

        Ok(PreMatchSnapshot {
            state_schema_version: STATE_SCHEMA_VERSION,
            work_id: id,
            work_created_at,
            title_guess,
            derived_title,
            title_provenance,
            media_kind,
            country_type,
            files,
            embedded_ids,
            evidence_class,
        })
    }
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
