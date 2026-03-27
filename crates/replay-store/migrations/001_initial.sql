CREATE TYPE call_type AS ENUM ('http', 'grpc', 'postgres', 'redis', 'function');
CREATE TYPE call_status AS ENUM ('completed', 'error', 'cancelled', 'timeout');

CREATE TABLE interactions (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    record_id    UUID        NOT NULL,
    parent_id    UUID,
    sequence     INT         NOT NULL,
    call_type    call_type   NOT NULL,
    fingerprint  TEXT        NOT NULL,
    request      JSONB       NOT NULL DEFAULT '{}',
    response     JSONB       NOT NULL DEFAULT '{}',
    duration_ms  BIGINT      NOT NULL DEFAULT 0,
    status       call_status NOT NULL DEFAULT 'completed',
    error        TEXT,
    recorded_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    build_hash   TEXT        NOT NULL DEFAULT '',
    service_name TEXT        NOT NULL DEFAULT ''
);

CREATE INDEX ix_interactions_record_id   ON interactions (record_id, sequence);
CREATE INDEX ix_interactions_fingerprint ON interactions (fingerprint, call_type);
CREATE INDEX ix_interactions_recorded_at ON interactions (recorded_at);

CREATE TABLE replay_runs (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    record_id   UUID        NOT NULL,
    new_build   TEXT        NOT NULL,
    old_build   TEXT        NOT NULL,
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    status      TEXT        NOT NULL DEFAULT 'running'
);

CREATE TABLE diff_results (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id         UUID NOT NULL REFERENCES replay_runs (id),
    interaction_id UUID NOT NULL REFERENCES interactions (id),
    field_path     TEXT NOT NULL,
    old_value      JSONB,
    new_value      JSONB,
    classification TEXT NOT NULL DEFAULT 'unknown'
);
