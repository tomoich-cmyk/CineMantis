-- Fill Wikidata IDs for person-oriented award categories.
-- These awards are usually stored on the recipient person with the film in
-- the "for work" qualifier, so category-level imports need the category QID.

UPDATE award_categories
SET wikidata_entity_id = CASE
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Academy Awards')
    AND name = 'Directing' THEN 'Q103360'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Festival de Cannes')
    AND name = 'Best Director' THEN 'Q510175'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Festival de Cannes')
    AND name = 'Best Screenplay' THEN 'Q978420'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Berlin International Film Festival')
    AND name = 'Silver Bear for Best Director' THEN 'Q706031'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Berlin International Film Festival')
    AND name = 'Silver Bear for Best Screenplay' THEN 'Q2285851'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'BAFTA Film Awards')
    AND name IN ('Best Director', 'Best Direction') THEN 'Q787131'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards')
    AND name = 'Best Director' THEN 'Q586356'
  ELSE wikidata_entity_id
END
WHERE award_body_id IN (
  SELECT id
  FROM award_bodies
  WHERE name IN (
    'Academy Awards',
    'Festival de Cannes',
    'Berlin International Film Festival',
    'BAFTA Film Awards',
    'Golden Globe Awards'
  )
)
AND name IN (
  'Directing',
  'Best Director',
  'Best Direction',
  'Best Screenplay',
  'Silver Bear for Best Director',
  'Silver Bear for Best Screenplay'
);
