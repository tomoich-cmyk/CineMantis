CREATE TABLE IF NOT EXISTS award_body_schedule_rules (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  award_body_id INTEGER NOT NULL REFERENCES award_bodies(id) ON DELETE CASCADE,
  nomination_month_start INTEGER CHECK (nomination_month_start BETWEEN 1 AND 12),
  nomination_month_end INTEGER CHECK (nomination_month_end BETWEEN 1 AND 12),
  result_month_start INTEGER CHECK (result_month_start BETWEEN 1 AND 12),
  result_month_end INTEGER CHECK (result_month_end BETWEEN 1 AND 12),
  ceremony_month INTEGER CHECK (ceremony_month BETWEEN 1 AND 12),
  data_source_url TEXT,
  wikidata_entity_id TEXT,
  notes TEXT,
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (award_body_id)
);

CREATE INDEX IF NOT EXISTS idx_award_body_schedule_rules_body
ON award_body_schedule_rules(award_body_id);

CREATE TABLE IF NOT EXISTS award_edition_data_status (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  award_edition_id INTEGER NOT NULL REFERENCES award_editions(id) ON DELETE CASCADE,
  data_imported_at TEXT,
  data_source TEXT CHECK (data_source IS NULL OR data_source IN ('wikidata','csv','json','manual','other')),
  is_data_complete INTEGER NOT NULL DEFAULT 0,
  note TEXT,
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (award_edition_id)
);

CREATE INDEX IF NOT EXISTS idx_award_edition_data_status_complete
ON award_edition_data_status(is_data_complete, data_imported_at);

INSERT INTO award_body_schedule_rules
(
  award_body_id,
  nomination_month_start,
  nomination_month_end,
  result_month_start,
  result_month_end,
  ceremony_month,
  data_source_url,
  wikidata_entity_id,
  notes
)
VALUES
((SELECT id FROM award_bodies WHERE name = 'Academy Awards'), 1, 1, 3, 3, 3, 'https://query.wikidata.org/', 'Q19020', 'Nominations are usually announced in January; ceremony/results are usually in March.'),
((SELECT id FROM award_bodies WHERE name = 'Festival de Cannes'), 5, 5, 5, 5, 5, 'https://query.wikidata.org/', 'Q16766', 'Official selection and awards are usually in May.'),
((SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival'), 7, 7, 9, 9, 9, 'https://query.wikidata.org/', 'Q11392', 'Lineup is usually announced in July; awards are usually in September.'),
((SELECT id FROM award_bodies WHERE name = 'Berlin International Film Festival'), 2, 2, 2, 2, 2, 'https://query.wikidata.org/', 'Q18778', 'Festival and awards are usually in February.'),
((SELECT id FROM award_bodies WHERE name = 'BAFTA Film Awards'), 1, 2, 2, 3, 2, 'https://query.wikidata.org/', 'Q732997', 'Nominations are usually in January; results are usually in February or March.'),
((SELECT id FROM award_bodies WHERE name = 'Cesar Awards'), 1, 2, 2, 3, 2, 'https://query.wikidata.org/', 'Q162455', 'Nominations are usually in winter; ceremony/results are usually in February or March.'),
((SELECT id FROM award_bodies WHERE name = 'Goya Awards'), 1, 2, 2, 2, 2, 'https://query.wikidata.org/', 'Q729541', 'Nominations often arrive in January; results are usually in February.'),
((SELECT id FROM award_bodies WHERE name = 'Japan Academy Film Prize'), 1, 1, 3, 3, 3, 'https://query.wikidata.org/', 'Q194258', 'Main nominations are usually in January; results are usually in March.'),
((SELECT id FROM award_bodies WHERE name = 'Sundance Film Festival'), 1, 1, 1, 2, 1, 'https://query.wikidata.org/', 'Q170484', 'Festival selections and awards are usually in January.'),
((SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards'), 12, 12, 1, 1, 1, 'https://query.wikidata.org/', 'Q1011547', 'Nominations often arrive in December; results are usually in January.'),
((SELECT id FROM award_bodies WHERE name = 'European Film Awards'), 11, 12, 1, 1, 1, 'https://query.wikidata.org/', 'Q684258', 'From 2026, the ceremony moves from December to January.')
ON CONFLICT(award_body_id) DO UPDATE SET
  nomination_month_start = excluded.nomination_month_start,
  nomination_month_end = excluded.nomination_month_end,
  result_month_start = excluded.result_month_start,
  result_month_end = excluded.result_month_end,
  ceremony_month = excluded.ceremony_month,
  data_source_url = excluded.data_source_url,
  wikidata_entity_id = excluded.wikidata_entity_id,
  notes = excluded.notes,
  updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now');
