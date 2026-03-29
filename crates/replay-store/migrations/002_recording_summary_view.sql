-- Recording summary view used by the ditto-server UI API.
-- Returns one row per record_id with entry-point metadata and interaction count.
CREATE OR REPLACE VIEW recording_summaries AS
SELECT
    record_id,
    COUNT(*)                                                                        AS interaction_count,
    MAX(recorded_at)                                                                AS recorded_at,
    COALESCE(MAX(build_hash),   '')                                                 AS build_hash,
    COALESCE(MAX(service_name), '')                                                 AS service_name,
    COALESCE(MAX(CASE WHEN sequence = 0 THEN request->>'method' END), '')           AS method,
    COALESCE(MAX(CASE WHEN sequence = 0 THEN request->>'path'   END), '')           AS path,
    COALESCE(MAX(CASE WHEN sequence = 0 THEN (response->>'status')::integer END), 0) AS status_code
FROM interactions
GROUP BY record_id;
