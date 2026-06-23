-- Collapse duplicate award imports/results caused by Wikidata carrying both
-- ceremony year and film year, or both nominee and winner statements.

WITH ranked_import_items AS (
  SELECT
    id,
    ROW_NUMBER() OVER (
      PARTITION BY
        job_id,
        raw_film_id,
        COALESCE(matched_award_category_id, -1),
        COALESCE(raw_award_name_en, '')
      ORDER BY
        CASE raw_result_type WHEN 'winner' THEN 0 WHEN 'nominee' THEN 1 ELSE 2 END,
        COALESCE(match_score, -1) DESC,
        raw_year DESC,
        CASE status WHEN 'approved' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END,
        id ASC
    ) AS rn
  FROM award_import_items
  WHERE raw_film_id IS NOT NULL
)
DELETE FROM award_import_items
WHERE id IN (
  SELECT id FROM ranked_import_items WHERE rn > 1
);

WITH ranked_results AS (
  SELECT
    war.id,
    ROW_NUMBER() OVER (
      PARTITION BY
        war.work_id,
        COALESCE(war.person_id, -1),
        war.award_body_id,
        war.award_category_id
      ORDER BY
        war.is_locked DESC,
        CASE war.result_type WHEN 'winner' THEN 0 WHEN 'nominee' THEN 1 ELSE 2 END,
        ae.year DESC,
        COALESCE(war.confidence, -1) DESC,
        war.id ASC
    ) AS rn
  FROM work_award_results war
  JOIN award_editions ae ON ae.id = war.award_edition_id
)
DELETE FROM work_award_results
WHERE id IN (
  SELECT id FROM ranked_results WHERE rn > 1
);
