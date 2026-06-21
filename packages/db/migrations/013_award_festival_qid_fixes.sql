-- Fix festival award category Wikidata QIDs verified against award received / nominated for movie statements.

UPDATE award_categories
SET wikidata_entity_id = 'Q209459'
WHERE award_body_id = (SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival')
  AND name = 'Golden Lion'
  AND (wikidata_entity_id IS NULL OR wikidata_entity_id <> 'Q209459');

UPDATE award_categories
SET wikidata_entity_id = 'Q154590'
WHERE award_body_id = (SELECT id FROM award_bodies WHERE name = 'Berlin International Film Festival')
  AND name = 'Golden Bear'
  AND (wikidata_entity_id IS NULL OR wikidata_entity_id <> 'Q154590');

UPDATE award_categories
SET wikidata_entity_id = 'Q1011509'
WHERE award_body_id = (SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards')
  AND name = 'Best Motion Picture - Drama'
  AND (wikidata_entity_id IS NULL OR wikidata_entity_id <> 'Q1011509');
