-- No row means the initial server configuration remains effective.
-- Consent evidence retains its immutable copy of every historical policy.
CREATE TABLE privacy_settings (
    id SMALLINT PRIMARY KEY CHECK (id = 1),
    policy_json TEXT NOT NULL,
    updated_at BIGINT NOT NULL,
    updated_by BIGINT REFERENCES administrators(id) ON DELETE SET NULL
);
