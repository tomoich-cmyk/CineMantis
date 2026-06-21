UPDATE award_body_schedule_rules
SET
  nomination_month_start = CASE
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival') THEN 7
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Goya Awards') THEN 1
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards') THEN 12
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards') THEN 11
    ELSE nomination_month_start
  END,
  nomination_month_end = CASE
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival') THEN 7
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Goya Awards') THEN 2
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards') THEN 12
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards') THEN 12
    ELSE nomination_month_end
  END,
  result_month_start = CASE
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Sundance Film Festival') THEN 1
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards') THEN 1
    ELSE result_month_start
  END,
  result_month_end = CASE
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Sundance Film Festival') THEN 2
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards') THEN 1
    ELSE result_month_end
  END,
  ceremony_month = CASE
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards') THEN 1
    ELSE ceremony_month
  END,
  wikidata_entity_id = CASE
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Academy Awards') THEN 'Q19020'
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Festival de Cannes') THEN 'Q16766'
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Venice International Film Festival') THEN 'Q11392'
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Berlin International Film Festival') THEN 'Q18778'
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'BAFTA Film Awards') THEN 'Q732997'
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Cesar Awards') THEN 'Q162455'
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Goya Awards') THEN 'Q729541'
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Japan Academy Film Prize') THEN 'Q194258'
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Sundance Film Festival') THEN 'Q170484'
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards') THEN 'Q1011547'
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards') THEN 'Q684258'
    ELSE wikidata_entity_id
  END,
  notes = CASE
    WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards') THEN 'From 2026, the ceremony moves from December to January.'
    ELSE notes
  END,
  updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now');

UPDATE award_bodies
SET wikidata_entity_id = (
  SELECT wikidata_entity_id
  FROM award_body_schedule_rules
  WHERE award_body_schedule_rules.award_body_id = award_bodies.id
)
WHERE EXISTS (
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
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards') AND name = 'Best Motion Picture - Drama' THEN 'Q1011509'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'Golden Globe Awards') AND name = 'Best Motion Picture - Musical or Comedy' THEN 'Q670282'
  WHEN award_body_id = (SELECT id FROM award_bodies WHERE name = 'European Film Awards') AND name = 'European Film' THEN 'Q777921'
  ELSE wikidata_entity_id
END
WHERE award_body_id IN (
  SELECT id
  FROM award_bodies
  WHERE name IN (
    'Academy Awards',
    'Festival de Cannes',
    'Venice International Film Festival',
    'Berlin International Film Festival',
    'BAFTA Film Awards',
    'Cesar Awards',
    'Goya Awards',
    'Japan Academy Film Prize',
    'Sundance Film Festival',
    'Golden Globe Awards',
    'European Film Awards'
  )
);
