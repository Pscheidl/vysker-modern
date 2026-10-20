ALTER TABLE navigation_items
    ADD COLUMN parent_id BIGINT REFERENCES navigation_items(id) ON DELETE RESTRICT,
    ADD CONSTRAINT navigation_parent_not_self CHECK (parent_id IS NULL OR parent_id <> id);

CREATE INDEX navigation_items_by_parent ON navigation_items(parent_id,sort_order,id);
