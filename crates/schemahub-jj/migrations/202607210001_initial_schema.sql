-- SchemaHub PostgreSQL storage baseline.
--
-- Every statement is adoption-safe: databases created by pre-migration
-- SchemaHub releases already contain these objects, so the first migrated
-- startup records this version without rewriting or dropping stored data.

CREATE TABLE IF NOT EXISTS objects (
    kind INTEGER NOT NULL,
    id BYTEA PRIMARY KEY,
    bytes BYTEA NOT NULL
);

CREATE INDEX IF NOT EXISTS objects_kind_idx ON objects (kind);

CREATE TABLE IF NOT EXISTS ops (
    repo TEXT NOT NULL,
    op_id BYTEA NOT NULL,
    op_bytes BYTEA NOT NULL,
    inserted_at BIGSERIAL NOT NULL,
    PRIMARY KEY (repo, op_id)
);

CREATE INDEX IF NOT EXISTS ops_repo_seq_idx ON ops (repo, inserted_at);

CREATE TABLE IF NOT EXISTS refs (
    repo TEXT NOT NULL,
    name TEXT NOT NULL,
    target BYTEA NOT NULL,
    PRIMARY KEY (repo, name)
);

CREATE TABLE IF NOT EXISTS resource_records (
    collection TEXT NOT NULL,
    record_key TEXT NOT NULL,
    record_bytes BYTEA NOT NULL,
    PRIMARY KEY (collection, record_key)
);
