-- Expand Wikidata IDs for major award/festival categories used by the
-- category-level import flow.
--
-- Some categories have more than one Wikidata item in use. Store them as a
-- whitespace-separated list; the importer expands this into VALUES ?award.

UPDATE award_categories
SET wikidata_entity_id = CASE
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Festival de Cannes')
    AND name = 'Grand Prix' THEN 'Q844804'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Festival de Cannes')
    AND name = 'Jury Prize' THEN 'Q164200'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Festival de Cannes')
    AND name = 'Best Actor' THEN 'Q586140'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Festival de Cannes')
    AND name = 'Best Actress' THEN 'Q840286'

  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival')
    AND name = 'Grand Jury Prize' THEN 'Q944480 Q20001886'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival')
    AND name = 'Silver Lion' THEN 'Q1337827'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival')
    AND name = 'Best Screenplay' THEN 'Q3405119 Q1444982'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival')
    AND name = 'Volpi Cup for Best Actor' THEN 'Q2089923'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival')
    AND name = 'Volpi Cup for Best Actress' THEN 'Q2089918'

  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'BAFTA Film Awards')
    AND name = 'Best Original Screenplay' THEN 'Q41375'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'BAFTA Film Awards')
    AND name = 'Best Adapted Screenplay' THEN 'Q739694'

  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Cesar Awards')
    AND name = 'Best Director' THEN 'Q24137'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Cesar Awards')
    AND name = 'Best Actor' THEN 'Q900494'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Cesar Awards')
    AND name = 'Best Actress' THEN 'Q24241'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Cesar Awards')
    AND name IN ('Best Screenplay', 'Best Original Screenplay') THEN 'Q545970 Q900369'

  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Goya Awards')
    AND name = 'Best Director' THEN 'Q1540553'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Goya Awards')
    AND name = 'Best Actor' THEN 'Q1520004'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Goya Awards')
    AND name = 'Best Actress' THEN 'Q1379415'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Goya Awards')
    AND name IN ('Best Screenplay', 'Best Original Screenplay') THEN 'Q2634446'

  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards')
    AND name = 'Best Screenplay' THEN 'Q849124'

  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards')
    AND name = 'European Director' THEN 'Q1377755'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards')
    AND name = 'European Screenwriter' THEN 'Q1377777'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards')
    AND name = 'European Actor' THEN 'Q932281'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards')
    AND name = 'European Actress' THEN 'Q1377738'
  ELSE wikidata_entity_id
END
WHERE award_body_id IN (
  SELECT id
  FROM award_bodies
  WHERE name IN (
    'Festival de Cannes',
    'Venice International Film Festival',
    'BAFTA Film Awards',
    'Cesar Awards',
    'Goya Awards',
    'Golden Globe Awards',
    'European Film Awards'
  )
);
