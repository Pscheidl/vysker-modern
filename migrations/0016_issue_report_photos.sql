CREATE TABLE issue_report_photos (
    report_id BIGINT NOT NULL REFERENCES issue_reports(id) ON DELETE CASCADE,
    position BIGINT NOT NULL CHECK (position > 0),
    photo BYTEA NOT NULL CHECK (octet_length(photo) BETWEEN 1 AND 15000000),
    PRIMARY KEY (report_id, position)
);

-- Existing reports retain their original photo as the first gallery item.
INSERT INTO issue_report_photos(report_id, position, photo)
    SELECT id, 1, photo FROM issue_reports WHERE photo IS NOT NULL;
ALTER TABLE issue_reports DROP COLUMN photo;
