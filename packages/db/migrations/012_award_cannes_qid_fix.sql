-- Fix Palme d'Or Wikidata QID.
-- Q179230 is "infinitive"; the Palme d'Or award item is Q179808.

UPDATE award_categories
SET wikidata_entity_id = 'Q179808'
WHERE award_body_id = (SELECT id FROM award_bodies WHERE name = 'Festival de Cannes')
  AND name = 'Palme d''Or'
  AND (wikidata_entity_id IS NULL OR wikidata_entity_id <> 'Q179808');
