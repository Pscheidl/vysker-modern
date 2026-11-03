-- Place the new default link between the overview and the notice board.
-- Preserve an existing calendar link, including a hidden or nested custom link,
-- and leave a full navigation for its administrator to reorganize.
INSERT INTO navigation_items(label,path,sort_order,visible)
SELECT 'Kalendář akcí','/kalendar',15,TRUE
WHERE NOT EXISTS (SELECT 1 FROM navigation_items WHERE path='/kalendar')
  AND (SELECT count(*) FROM navigation_items WHERE parent_id IS NULL) < 20
  AND (SELECT count(*) FROM navigation_items) < 60;
