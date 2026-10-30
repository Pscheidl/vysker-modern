ALTER TABLE administrators
    ADD COLUMN display_name TEXT,
    ADD CONSTRAINT administrator_display_name_length
        CHECK (display_name IS NULL OR (char_length(display_name) BETWEEN 1 AND 120 AND display_name = btrim(display_name)));
