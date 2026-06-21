CREATE TABLE IF NOT EXISTS award_import_jobs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  award_body_id INTEGER NOT NULL REFERENCES award_bodies(id) ON DELETE CASCADE,
  edition_year INTEGER,
  source TEXT NOT NULL DEFAULT 'wikidata' CHECK (source IN ('wikidata','csv','json','manual','other')),
  source_detail TEXT,
  wikidata_award_id TEXT,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','running','done','error','cancelled')),
  total_items INTEGER,
  matched_items INTEGER,
  error_message TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_award_import_jobs_body
ON award_import_jobs(award_body_id, edition_year, created_at);

CREATE INDEX IF NOT EXISTS idx_award_import_jobs_status
ON award_import_jobs(status, created_at);

CREATE TABLE IF NOT EXISTS award_import_items (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id INTEGER NOT NULL REFERENCES award_import_jobs(id) ON DELETE CASCADE,
  raw_film_id TEXT,
  raw_title_en TEXT,
  raw_title_ja TEXT,
  raw_imdb_id TEXT,
  raw_tmdb_id TEXT,
  raw_award_name_en TEXT,
  raw_award_name_ja TEXT,
  raw_year INTEGER,
  raw_result_type TEXT NOT NULL CHECK (raw_result_type IN ('winner','nominee','unknown')),
  raw_source_url TEXT,
  matched_work_id INTEGER REFERENCES works(id) ON DELETE SET NULL,
  matched_award_category_id INTEGER REFERENCES award_categories(id) ON DELETE SET NULL,
  match_method TEXT CHECK (match_method IS NULL OR match_method IN ('tmdb_id','imdb_id','title_exact','title_fuzzy','manual')),
  match_score REAL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','approved','rejected','imported','error')),
  approved_at TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE(job_id, raw_film_id, raw_year, raw_result_type, raw_award_name_en)
);

CREATE INDEX IF NOT EXISTS idx_award_import_items_job
ON award_import_items(job_id, status);

CREATE INDEX IF NOT EXISTS idx_award_import_items_match
ON award_import_items(matched_work_id, matched_award_category_id, raw_year);

CREATE INDEX IF NOT EXISTS idx_award_import_items_external_ids
ON award_import_items(raw_tmdb_id, raw_imdb_id);

CREATE TABLE IF NOT EXISTS work_award_match_candidates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  import_item_id INTEGER NOT NULL REFERENCES award_import_items(id) ON DELETE CASCADE,
  work_id INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
  match_score REAL NOT NULL,
  match_method TEXT NOT NULL CHECK (match_method IN ('tmdb_id','imdb_id','title_exact','title_fuzzy','manual')),
  is_selected INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE(import_item_id, work_id)
);

CREATE INDEX IF NOT EXISTS idx_work_award_match_candidates_item
ON work_award_match_candidates(import_item_id, is_selected, match_score);

CREATE INDEX IF NOT EXISTS idx_work_award_match_candidates_work
ON work_award_match_candidates(work_id);

ALTER TABLE award_bodies ADD COLUMN wikidata_entity_id TEXT;
ALTER TABLE award_categories ADD COLUMN wikidata_entity_id TEXT;

UPDATE award_bodies
SET wikidata_entity_id = (
  SELECT wikidata_entity_id
  FROM award_body_schedule_rules
  WHERE award_body_schedule_rules.award_body_id = award_bodies.id
)
WHERE wikidata_entity_id IS NULL
  AND EXISTS (
    SELECT 1
    FROM award_body_schedule_rules
    WHERE award_body_schedule_rules.award_body_id = award_bodies.id
      AND award_body_schedule_rules.wikidata_entity_id IS NOT NULL
  );

UPDATE award_categories
SET wikidata_entity_id = CASE
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Academy Awards') AND name = 'Best Picture' THEN 'Q102427'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Festival de Cannes') AND name = 'Palme d''Or' THEN 'Q179808'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival') AND name = 'Golden Lion' THEN 'Q11384'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Berlin International Film Festival') AND name = 'Golden Bear' THEN 'Q183614'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'BAFTA Film Awards') AND name = 'Best Film' THEN 'Q139184'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Cesar Awards') AND name = 'Best Film' THEN 'Q645595'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Goya Awards') AND name = 'Best Film' THEN 'Q1467554'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Japan Academy Film Prize') AND name = 'Picture of the Year' THEN 'Q378567'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Sundance Film Festival') AND name = 'Grand Jury Prize: U.S. Dramatic' THEN 'Q15974895'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards') AND name = 'Best Motion Picture - Drama' THEN 'Q1011573'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards') AND name = 'Best Motion Picture - Musical or Comedy' THEN 'Q670282'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards') AND name = 'European Film' THEN 'Q777921'
  ELSE wikidata_entity_id
END
WHERE wikidata_entity_id IS NULL;
