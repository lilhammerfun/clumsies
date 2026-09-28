-- A rejected review stays closed while its editable proposals can enter a new review.
ALTER TABLE reviews ADD COLUMN closed_drafts JSONB;
ALTER TABLE reviews DROP CONSTRAINT reviews_draft_id_key;
CREATE UNIQUE INDEX reviews_active_draft_idx ON reviews (draft_id)
    WHERE status IN ('open', 'approved');
ALTER TABLE review_drafts DROP CONSTRAINT review_drafts_draft_id_key;
CREATE INDEX review_drafts_draft_idx ON review_drafts (draft_id);

-- Preserve the last available state of existing rejected reviews before future edits.
UPDATE reviews r SET closed_drafts = (
    SELECT jsonb_agg(jsonb_build_object(
        'draft', jsonb_build_object(
            'draft_id', d.draft_id, 'project_id', d.project_id,
            'base_commit_id', d.base_commit_id,
            'author', jsonb_build_object('user_id', u.user_id, 'email', u.email,
                'display_name', u.display_name, 'avatar_url', u.avatar_url, 'role', u.role),
            'title', d.title, 'description', d.description,
            'resource', jsonb_build_object('scope', d.resource_scope,
                'id', d.target_id, 'path', d.path),
            'status', d.status, 'version', d.version,
            'coordination', jsonb_build_object('freshness', 'current',
                'current_commit_id', d.base_commit_id, 'has_upstream_resource_changes', false,
                'reconciliation', 'unknown', 'candidate_id', NULL, 'auto_rebased', false),
            'created_at', d.created_at, 'updated_at', d.updated_at),
        'operations', COALESCE((
            SELECT jsonb_agg(jsonb_build_object(
                'operation_id', o.operation_id, 'action', o.action,
                'resource', jsonb_build_object('scope', o.resource_scope,
                    'id', o.target_id, 'path', o.path),
                'content', o.content, 'new_path', o.new_path, 'created_at', o.created_at
            ) ORDER BY o.ordinal) FROM draft_operations o WHERE o.draft_id = d.draft_id
        ), '[]'::jsonb)
    ) ORDER BY rd.ordinal)
    FROM review_drafts rd JOIN drafts d USING (draft_id)
    JOIN users u ON u.user_id = d.author_user_id
    WHERE rd.review_id = r.review_id
) WHERE r.status = 'rejected';
