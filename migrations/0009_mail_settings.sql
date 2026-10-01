-- Absence of this singleton selects SMTP from the server environment.
-- The application stores only authenticated ciphertext, never an app password.
CREATE TABLE mail_settings (
    id SMALLINT PRIMARY KEY CHECK (id = 1),
    username TEXT NOT NULL,
    sender_name TEXT NOT NULL,
    password_encrypted BYTEA NOT NULL,
    updated_at BIGINT NOT NULL,
    updated_by BIGINT REFERENCES administrators(id) ON DELETE SET NULL
);
