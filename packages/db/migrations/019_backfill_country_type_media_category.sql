-- 既存の照合済み作品に country_type / media_category を埋める
-- tmdb_media_type が確定している作品のみ対象

UPDATE works
SET media_category = CASE tmdb_media_type
    WHEN 'movie' THEN 'movie'
    WHEN 'tv'    THEN 'drama'
    ELSE media_category
END
WHERE tmdb_id IS NOT NULL
  AND tmdb_media_type IN ('movie', 'tv')
  AND COALESCE(media_category, 'other') = 'other';

UPDATE works
SET country_type = CASE
    WHEN country_json LIKE '%JP%' THEN 'domestic'
    ELSE 'foreign'
END
WHERE tmdb_id IS NOT NULL
  AND country_json IS NOT NULL
  AND country_json != ''
  AND country_json != '[]'
  AND COALESCE(country_type, 'unknown') = 'unknown';
