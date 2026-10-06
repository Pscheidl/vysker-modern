-- Share preference matching between native publication, imports and delivery.
-- Match existing records only, and never interpret an invalid publication as all mail.
CREATE FUNCTION subscription_matches_publication(
    subscriber BIGINT, notice BIGINT, document BIGINT
) RETURNS BOOLEAN
LANGUAGE SQL STABLE
AS $$
    SELECT EXISTS (
        SELECT 1 FROM subscribers s
        WHERE s.id=subscriber
          AND (
            (notice IS NOT NULL AND document IS NULL AND EXISTS (
                SELECT 1 FROM notices n WHERE n.id=notice
                  AND CASE WHEN n.category_id IS NULL
                      THEN s.uncategorized_notices
                      ELSE s.all_notice_categories OR n.category_id=ANY(s.notice_category_ids)
                  END
            ))
            OR (notice IS NULL AND document IS NOT NULL AND s.documents
                AND EXISTS (SELECT 1 FROM documents d WHERE d.id=document))
          )
    )
$$;
