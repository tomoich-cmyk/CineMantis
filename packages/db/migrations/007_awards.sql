CREATE TABLE IF NOT EXISTS award_bodies (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  display_name_ja TEXT NOT NULL,
  original_name TEXT,
  sort_name TEXT,
  body_type TEXT NOT NULL CHECK (body_type IN ('industry_award','film_festival','national_academy','regional_award','critics_award','other')),
  prestige_tier TEXT NOT NULL CHECK (prestige_tier IN ('S','A','B','C')),
  award_scope TEXT NOT NULL CHECK (award_scope IN ('global_industry','major_festival','national_academy','indie_discovery','awards_season','regional','critics','other')),
  country TEXT,
  city TEXT,
  official_url TEXT,
  is_active INTEGER NOT NULL DEFAULT 1,
  display_order INTEGER NOT NULL DEFAULT 999,
  note TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (name)
);

CREATE INDEX IF NOT EXISTS idx_award_bodies_tier ON award_bodies(prestige_tier, display_order);
CREATE INDEX IF NOT EXISTS idx_award_bodies_scope ON award_bodies(award_scope);

CREATE TABLE IF NOT EXISTS award_editions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  award_body_id INTEGER NOT NULL REFERENCES award_bodies(id) ON DELETE CASCADE,
  year INTEGER NOT NULL,
  edition_no INTEGER,
  start_date TEXT,
  end_date TEXT,
  ceremony_date TEXT,
  official_url TEXT,
  note TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (award_body_id, year)
);

CREATE INDEX IF NOT EXISTS idx_award_editions_body_year ON award_editions(award_body_id, year);

CREATE TABLE IF NOT EXISTS award_categories (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  award_body_id INTEGER NOT NULL REFERENCES award_bodies(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  display_name_ja TEXT NOT NULL,
  original_name TEXT,
  category_type TEXT NOT NULL CHECK (category_type IN ('best_picture','grand_prize','director','actor','actress','supporting_actor','supporting_actress','screenplay','international_feature','animation','documentary','audience','technical','special','other')),
  target_type TEXT NOT NULL CHECK (target_type IN ('work','person','work_and_person')),
  is_top_prize INTEGER NOT NULL DEFAULT 0,
  is_major_category INTEGER NOT NULL DEFAULT 1,
  display_order INTEGER NOT NULL DEFAULT 999,
  note TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (award_body_id, name)
);

CREATE INDEX IF NOT EXISTS idx_award_categories_body ON award_categories(award_body_id, display_order);
CREATE INDEX IF NOT EXISTS idx_award_categories_type ON award_categories(category_type);

CREATE TABLE IF NOT EXISTS work_award_results (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  work_id INTEGER NOT NULL REFERENCES works(id) ON DELETE CASCADE,
  person_id INTEGER REFERENCES persons(id) ON DELETE SET NULL,
  award_body_id INTEGER NOT NULL REFERENCES award_bodies(id) ON DELETE CASCADE,
  award_edition_id INTEGER REFERENCES award_editions(id) ON DELETE SET NULL,
  award_category_id INTEGER NOT NULL REFERENCES award_categories(id) ON DELETE CASCADE,
  result_type TEXT NOT NULL CHECK (result_type IN ('winner','nominee','shortlisted','special_mention','selection','unknown')),
  section_name TEXT,
  source_url TEXT,
  confidence REAL,
  is_locked INTEGER NOT NULL DEFAULT 0,
  note TEXT,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  UNIQUE (work_id, person_id, award_body_id, award_edition_id, award_category_id, result_type)
);

CREATE INDEX IF NOT EXISTS idx_work_award_results_work ON work_award_results(work_id);
CREATE INDEX IF NOT EXISTS idx_work_award_results_body ON work_award_results(award_body_id);
CREATE INDEX IF NOT EXISTS idx_work_award_results_category ON work_award_results(award_category_id);
CREATE INDEX IF NOT EXISTS idx_work_award_results_result ON work_award_results(result_type);

CREATE TRIGGER IF NOT EXISTS trg_award_bodies_updated_at
AFTER UPDATE ON award_bodies
BEGIN
  UPDATE award_bodies SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_award_editions_updated_at
AFTER UPDATE ON award_editions
BEGIN
  UPDATE award_editions SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_award_categories_updated_at
AFTER UPDATE ON award_categories
BEGIN
  UPDATE award_categories SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_work_award_results_updated_at
AFTER UPDATE ON work_award_results
BEGIN
  UPDATE work_award_results SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = NEW.id;
END;

INSERT OR IGNORE INTO award_bodies
(name, display_name_ja, original_name, sort_name, body_type, prestige_tier, award_scope, country, city, official_url, display_order)
VALUES
('Academy Awards', 'アカデミー賞', 'Academy Awards', 'Academy Awards', 'industry_award', 'S', 'global_industry', 'US', 'Los Angeles', 'https://www.oscars.org/oscars', 10),
('Festival de Cannes', 'カンヌ国際映画祭', 'Festival de Cannes', 'Cannes', 'film_festival', 'S', 'major_festival', 'FR', 'Cannes', 'https://www.festival-cannes.com/', 20),
('Venice International Film Festival', 'ヴェネツィア国際映画祭', 'Mostra Internazionale d''Arte Cinematografica', 'Venice', 'film_festival', 'S', 'major_festival', 'IT', 'Venice', 'https://www.labiennale.org/en/cinema', 30),
('Berlin International Film Festival', 'ベルリン国際映画祭', 'Internationale Filmfestspiele Berlin', 'Berlin', 'film_festival', 'S', 'major_festival', 'DE', 'Berlin', 'https://www.berlinale.de/', 40),
('BAFTA Film Awards', '英国アカデミー賞', 'BAFTA Film Awards', 'BAFTA', 'national_academy', 'A', 'national_academy', 'GB', 'London', 'https://www.bafta.org/awards/film', 50),
('Cesar Awards', 'セザール賞', 'Cesar du cinema', 'Cesar', 'national_academy', 'A', 'national_academy', 'FR', 'Paris', NULL, 60),
('Goya Awards', 'ゴヤ賞', 'Premios Goya', 'Goya', 'national_academy', 'A', 'national_academy', 'ES', 'Madrid', NULL, 70),
('Japan Academy Film Prize', '日本アカデミー賞', '日本アカデミー賞', 'Japan Academy', 'national_academy', 'A', 'national_academy', 'JP', 'Tokyo', 'https://www.japan-academy-prize.jp/', 80),
('Sundance Film Festival', 'サンダンス映画祭', 'Sundance Film Festival', 'Sundance', 'film_festival', 'B', 'indie_discovery', 'US', 'Park City', 'https://www.sundance.org/', 90),
('Golden Globe Awards', 'ゴールデングローブ賞', 'Golden Globe Awards', 'Golden Globes', 'industry_award', 'B', 'awards_season', 'US', 'Los Angeles', 'https://goldenglobes.com/', 100),
('European Film Awards', 'ヨーロッパ映画賞', 'European Film Awards', 'European Film Awards', 'regional_award', 'B', 'regional', NULL, NULL, 'https://www.europeanfilmawards.eu/', 110);

INSERT OR IGNORE INTO award_categories
(award_body_id, name, display_name_ja, original_name, category_type, target_type, is_top_prize, is_major_category, display_order)
SELECT ab.id, c.name, c.display_name_ja, c.original_name, c.category_type, c.target_type, c.is_top_prize, c.is_major_category, c.display_order
FROM award_bodies ab
JOIN (
  SELECT 'Academy Awards' body, 'Best Picture' name, '作品賞' display_name_ja, 'Best Picture' original_name, 'best_picture' category_type, 'work' target_type, 1 is_top_prize, 1 is_major_category, 10 display_order UNION ALL
  SELECT 'Academy Awards','Directing','監督賞','Directing','director','person',0,1,20 UNION ALL
  SELECT 'Academy Awards','International Feature Film','国際長編映画賞','International Feature Film','international_feature','work',0,1,30 UNION ALL
  SELECT 'Academy Awards','Original Screenplay','脚本賞','Original Screenplay','screenplay','work_and_person',0,1,40 UNION ALL
  SELECT 'Academy Awards','Adapted Screenplay','脚色賞','Adapted Screenplay','screenplay','work_and_person',0,1,50 UNION ALL
  SELECT 'Academy Awards','Actor in a Leading Role','主演男優賞','Actor in a Leading Role','actor','person',0,1,60 UNION ALL
  SELECT 'Academy Awards','Actress in a Leading Role','主演女優賞','Actress in a Leading Role','actress','person',0,1,70 UNION ALL
  SELECT 'Academy Awards','Actor in a Supporting Role','助演男優賞','Actor in a Supporting Role','supporting_actor','person',0,1,80 UNION ALL
  SELECT 'Academy Awards','Actress in a Supporting Role','助演女優賞','Actress in a Supporting Role','supporting_actress','person',0,1,90 UNION ALL
  SELECT 'Academy Awards','Animated Feature Film','長編アニメ映画賞','Animated Feature Film','animation','work',0,1,100 UNION ALL
  SELECT 'Academy Awards','Documentary Feature Film','長編ドキュメンタリー賞','Documentary Feature Film','documentary','work',0,1,110 UNION ALL

  SELECT 'Festival de Cannes','Palme d''Or','パルムドール','Palme d''Or','grand_prize','work',1,1,10 UNION ALL
  SELECT 'Festival de Cannes','Grand Prix','グランプリ','Grand Prix','grand_prize','work',0,1,20 UNION ALL
  SELECT 'Festival de Cannes','Jury Prize','審査員賞','Jury Prize','special','work',0,1,30 UNION ALL
  SELECT 'Festival de Cannes','Best Director','監督賞','Best Director','director','person',0,1,40 UNION ALL
  SELECT 'Festival de Cannes','Best Screenplay','脚本賞','Best Screenplay','screenplay','work_and_person',0,1,50 UNION ALL
  SELECT 'Festival de Cannes','Best Actor','男優賞','Best Actor','actor','person',0,1,60 UNION ALL
  SELECT 'Festival de Cannes','Best Actress','女優賞','Best Actress','actress','person',0,1,70 UNION ALL

  SELECT 'Venice International Film Festival','Golden Lion','金獅子賞','Golden Lion','grand_prize','work',1,1,10 UNION ALL
  SELECT 'Venice International Film Festival','Grand Jury Prize','審査員大賞','Grand Jury Prize','grand_prize','work',0,1,20 UNION ALL
  SELECT 'Venice International Film Festival','Silver Lion','銀獅子賞','Silver Lion','director','person',0,1,30 UNION ALL
  SELECT 'Venice International Film Festival','Volpi Cup for Best Actor','男優賞','Volpi Cup for Best Actor','actor','person',0,1,40 UNION ALL
  SELECT 'Venice International Film Festival','Volpi Cup for Best Actress','女優賞','Volpi Cup for Best Actress','actress','person',0,1,50 UNION ALL
  SELECT 'Venice International Film Festival','Best Screenplay','脚本賞','Best Screenplay','screenplay','work_and_person',0,1,60 UNION ALL

  SELECT 'Berlin International Film Festival','Golden Bear','金熊賞','Golden Bear','grand_prize','work',1,1,10 UNION ALL
  SELECT 'Berlin International Film Festival','Silver Bear Grand Jury Prize','銀熊審査員大賞','Silver Bear Grand Jury Prize','grand_prize','work',0,1,20 UNION ALL
  SELECT 'Berlin International Film Festival','Silver Bear Jury Prize','銀熊審査員賞','Silver Bear Jury Prize','special','work',0,1,30 UNION ALL
  SELECT 'Berlin International Film Festival','Silver Bear for Best Director','監督賞','Silver Bear for Best Director','director','person',0,1,40 UNION ALL
  SELECT 'Berlin International Film Festival','Silver Bear for Best Leading Performance','主演賞','Silver Bear for Best Leading Performance','actor','person',0,1,50 UNION ALL
  SELECT 'Berlin International Film Festival','Silver Bear for Best Screenplay','脚本賞','Silver Bear for Best Screenplay','screenplay','work_and_person',0,1,60 UNION ALL

  SELECT body, 'Best Film', '作品賞', 'Best Film', 'best_picture', 'work', 1, 1, 10 FROM (
    SELECT 'BAFTA Film Awards' body UNION ALL SELECT 'Cesar Awards' UNION ALL SELECT 'Goya Awards' UNION ALL SELECT 'Japan Academy Film Prize' UNION ALL SELECT 'European Film Awards'
  ) UNION ALL
  SELECT body, 'Best Director', '監督賞', 'Best Director', 'director', 'person', 0, 1, 20 FROM (
    SELECT 'BAFTA Film Awards' body UNION ALL SELECT 'Cesar Awards' UNION ALL SELECT 'Goya Awards' UNION ALL SELECT 'Japan Academy Film Prize' UNION ALL SELECT 'European Film Awards'
  ) UNION ALL
  SELECT body, 'Best Actor', '主演男優賞', 'Best Actor', 'actor', 'person', 0, 1, 30 FROM (
    SELECT 'BAFTA Film Awards' body UNION ALL SELECT 'Cesar Awards' UNION ALL SELECT 'Goya Awards' UNION ALL SELECT 'Japan Academy Film Prize' UNION ALL SELECT 'European Film Awards'
  ) UNION ALL
  SELECT body, 'Best Actress', '主演女優賞', 'Best Actress', 'actress', 'person', 0, 1, 40 FROM (
    SELECT 'BAFTA Film Awards' body UNION ALL SELECT 'Cesar Awards' UNION ALL SELECT 'Goya Awards' UNION ALL SELECT 'Japan Academy Film Prize' UNION ALL SELECT 'European Film Awards'
  ) UNION ALL
  SELECT body, 'Best Screenplay', '脚本賞', 'Best Screenplay', 'screenplay', 'work_and_person', 0, 1, 50 FROM (
    SELECT 'BAFTA Film Awards' body UNION ALL SELECT 'Cesar Awards' UNION ALL SELECT 'Goya Awards' UNION ALL SELECT 'Japan Academy Film Prize' UNION ALL SELECT 'European Film Awards'
  ) UNION ALL
  SELECT body, 'Best International Film', '国際映画賞', 'Best International Film', 'international_feature', 'work', 0, 1, 60 FROM (
    SELECT 'BAFTA Film Awards' body UNION ALL SELECT 'Cesar Awards' UNION ALL SELECT 'Goya Awards' UNION ALL SELECT 'Japan Academy Film Prize' UNION ALL SELECT 'European Film Awards'
  ) UNION ALL

  SELECT 'Sundance Film Festival','Grand Jury Prize: U.S. Dramatic','審査員大賞 U.S. Dramatic','Grand Jury Prize: U.S. Dramatic','grand_prize','work',1,1,10 UNION ALL
  SELECT 'Sundance Film Festival','Grand Jury Prize: U.S. Documentary','審査員大賞 U.S. Documentary','Grand Jury Prize: U.S. Documentary','documentary','work',0,1,20 UNION ALL
  SELECT 'Sundance Film Festival','Grand Jury Prize: World Cinema Dramatic','審査員大賞 World Cinema Dramatic','Grand Jury Prize: World Cinema Dramatic','grand_prize','work',0,1,30 UNION ALL
  SELECT 'Sundance Film Festival','Grand Jury Prize: World Cinema Documentary','審査員大賞 World Cinema Documentary','Grand Jury Prize: World Cinema Documentary','documentary','work',0,1,40 UNION ALL
  SELECT 'Sundance Film Festival','Audience Award','観客賞','Audience Award','audience','work',0,1,50 UNION ALL

  SELECT 'Golden Globe Awards','Best Motion Picture - Drama','作品賞 ドラマ部門','Best Motion Picture - Drama','best_picture','work',1,1,10 UNION ALL
  SELECT 'Golden Globe Awards','Best Motion Picture - Musical or Comedy','作品賞 ミュージカル・コメディ部門','Best Motion Picture - Musical or Comedy','best_picture','work',1,1,20 UNION ALL
  SELECT 'Golden Globe Awards','Best Director','監督賞','Best Director','director','person',0,1,30 UNION ALL
  SELECT 'Golden Globe Awards','Best Screenplay','脚本賞','Best Screenplay','screenplay','work_and_person',0,1,40 UNION ALL
  SELECT 'Golden Globe Awards','Best Motion Picture - Non-English Language','非英語映画賞','Best Motion Picture - Non-English Language','international_feature','work',0,1,50 UNION ALL
  SELECT 'Golden Globe Awards','Best Actor','男優賞','Best Actor','actor','person',0,1,60 UNION ALL
  SELECT 'Golden Globe Awards','Best Actress','女優賞','Best Actress','actress','person',0,1,70
) c ON c.body = ab.name;
