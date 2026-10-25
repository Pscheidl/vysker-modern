-- Private sender context. Browser headers and the optional name remain unverified.
ALTER TABLE issue_reports
    ADD COLUMN reporter_email TEXT CHECK (char_length(reporter_email) BETWEEN 1 AND 254),
    ADD COLUMN reporter_phone TEXT CHECK (char_length(reporter_phone) BETWEEN 1 AND 40),
    ADD COLUMN request_user_agent TEXT,
    ADD COLUMN request_accept_language TEXT,
    ADD COLUMN client_timezone TEXT,
    ADD COLUMN browser_id_hash TEXT,
    ADD COLUMN browser_fingerprint_hash TEXT,
    ADD COLUMN authenticated_admin_id BIGINT REFERENCES administrators(id) ON DELETE SET NULL,
    ADD COLUMN authenticated_admin_email TEXT,
    ADD COLUMN metadata_recorded_at BIGINT,
    ADD COLUMN metadata_expires_at BIGINT;
-- Existing IP addresses receive a full grace period after this deployment.
UPDATE issue_reports SET metadata_expires_at=EXTRACT(EPOCH FROM CURRENT_TIMESTAMP)::BIGINT + 30*86400;

-- This random per-site key survives application restarts. Never expose it to clients.
CREATE TABLE report_abuse_settings (
    id BIGINT PRIMARY KEY CHECK (id=1),
    secret TEXT NOT NULL
);
CREATE TABLE report_submission_tokens (
    token_hash TEXT PRIMARY KEY,
    browser_hash TEXT NOT NULL,
    issuance_ip_hash TEXT NOT NULL,
    expires_at BIGINT NOT NULL,
    retain_until BIGINT NOT NULL,
    payload_hash TEXT,
    report_id BIGINT REFERENCES issue_reports(id) ON DELETE CASCADE
);
CREATE INDEX report_submission_tokens_expiry ON report_submission_tokens(retain_until);
CREATE TABLE report_abuse_rates (
    scope TEXT NOT NULL,
    identity_hash TEXT NOT NULL,
    bucket BIGINT NOT NULL,
    hits BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    PRIMARY KEY(scope,identity_hash,bucket)
);
CREATE INDEX report_abuse_rates_expiry ON report_abuse_rates(expires_at);
CREATE TABLE report_abuse_events (
    report_id BIGINT PRIMARY KEY REFERENCES issue_reports(id) ON DELETE CASCADE,
    browser_hash TEXT NOT NULL,
    ip_hash TEXT NOT NULL,
    created_at BIGINT NOT NULL
);
CREATE INDEX report_abuse_events_browser ON report_abuse_events(browser_hash,created_at);
CREATE INDEX report_abuse_events_ip ON report_abuse_events(ip_hash,created_at);
CREATE INDEX report_abuse_events_expiry ON report_abuse_events(created_at);
CREATE TABLE report_submission_duplicates (
    browser_hash TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    report_id BIGINT NOT NULL REFERENCES issue_reports(id) ON DELETE CASCADE,
    expires_at BIGINT NOT NULL,
    PRIMARY KEY(browser_hash,payload_hash)
);
CREATE INDEX report_submission_duplicates_expiry ON report_submission_duplicates(expires_at);

-- Pending notifications created by older releases also lose the private sender line.
UPDATE report_mail_queue
SET body=regexp_replace(body,E'\nKdo nahlásil: [^\n]*\n',E'\n')
WHERE sent_at IS NULL AND body LIKE 'Nové hlášení závady č. %';
