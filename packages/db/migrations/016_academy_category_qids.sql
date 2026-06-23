-- Fill Wikidata IDs for Academy Awards categories that were present in the
-- master data but could not be imported category-by-category.

UPDATE award_categories
SET wikidata_entity_id = CASE
  WHEN name = 'Original Screenplay' THEN 'Q41417'
  WHEN name = 'Adapted Screenplay' THEN 'Q107258'
  WHEN name = 'Actor in a Leading Role' THEN 'Q103916'
  WHEN name = 'Actress in a Leading Role' THEN 'Q103618'
  WHEN name = 'Actor in a Supporting Role' THEN 'Q106291'
  WHEN name = 'Actress in a Supporting Role' THEN 'Q106301'
  WHEN name = 'International Feature Film' THEN 'Q105304'
  WHEN name = 'Animated Feature Film' THEN 'Q106800'
  WHEN name = 'Documentary Feature Film' THEN 'Q111332'
  ELSE wikidata_entity_id
END
WHERE award_body_id = (SELECT id FROM award_bodies WHERE name = 'Academy Awards')
  AND name IN (
    'Original Screenplay',
    'Adapted Screenplay',
    'Actor in a Leading Role',
    'Actress in a Leading Role',
    'Actor in a Supporting Role',
    'Actress in a Supporting Role',
    'International Feature Film',
    'Animated Feature Film',
    'Documentary Feature Film'
  );
