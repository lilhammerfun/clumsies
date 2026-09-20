-- Remove discarded members from pending reviews while preserving discarded draft history.
-- Membership changes invalidate approval and advance the Review version so stale decisions fail.
WITH affected AS (
    SELECT r.review_id,
           (SELECT rd.draft_id FROM review_drafts rd JOIN drafts d USING (draft_id)
            WHERE rd.review_id = r.review_id AND d.status != 'discarded'
            ORDER BY rd.ordinal LIMIT 1) AS next_primary
    FROM reviews r
    WHERE r.status != 'merged'
      AND EXISTS (SELECT 1 FROM review_drafts rd JOIN drafts d USING (draft_id)
                  WHERE rd.review_id = r.review_id AND d.status = 'discarded')
)
UPDATE reviews r
SET draft_id = COALESCE(a.next_primary, r.draft_id),
    status = CASE WHEN a.next_primary IS NULL THEN 'rejected'
                  WHEN r.status = 'approved' THEN 'open' ELSE r.status END,
    version = r.version + 1,
    approved_result_hash = NULL,
    decision_body = CASE WHEN a.next_primary IS NULL THEN 'Draft discarded.' ELSE NULL END,
    decided_by_user_id = NULL, decided_at = NULL, updated_at = now()
FROM affected a WHERE r.review_id = a.review_id;

-- Keep the final discarded proposal as the closed Review's historical record.
DELETE FROM review_drafts rd USING drafts d, reviews r
WHERE rd.draft_id = d.draft_id AND rd.review_id = r.review_id
  AND r.status != 'merged' AND d.status = 'discarded'
  AND EXISTS (SELECT 1 FROM review_drafts kept JOIN drafts live USING (draft_id)
              WHERE kept.review_id = r.review_id AND live.status != 'discarded');
