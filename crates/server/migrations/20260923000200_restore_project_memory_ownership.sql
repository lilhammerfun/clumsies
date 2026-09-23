-- Restore publication without changing existing resource identities, content, or drafts.
ALTER TABLE resources DROP CONSTRAINT resources_no_active_project_authority;
ALTER TABLE drafts DROP CONSTRAINT drafts_no_active_project_authority;

-- Origin is independent of paths and retained after the selected source is archived.
ALTER TABLE resources ADD COLUMN org_source JSONB;
ALTER TABLE tree_entries ADD COLUMN org_source JSONB;
ALTER TABLE resources ADD CONSTRAINT project_adaptation_source CHECK (
    org_source IS NULL OR (scope = 'project' AND jsonb_typeof(org_source) = 'object'
      AND org_source ? 'resource_id' AND org_source ? 'commit_id')
);
CREATE UNIQUE INDEX active_project_adaptation_source
    ON resources (project_id, (org_source->>'resource_id'))
    WHERE scope = 'project' AND status = 'active' AND org_source IS NOT NULL;
