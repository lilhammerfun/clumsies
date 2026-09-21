-- Personal welcome and access notices outlive project membership; source notices still require it.
ALTER TABLE inbox_notifications
    ADD COLUMN org_id TEXT REFERENCES orgs(org_id) ON DELETE CASCADE,
    ADD COLUMN project_name_snapshot TEXT,
    ADD COLUMN body TEXT,
    ADD COLUMN previous_role TEXT,
    ADD COLUMN new_role TEXT,
    ALTER COLUMN project_id DROP NOT NULL,
    DROP CONSTRAINT inbox_notifications_project_id_fkey,
    ADD CONSTRAINT inbox_notifications_project_id_fkey
        FOREIGN KEY (project_id) REFERENCES projects(project_id) ON DELETE SET NULL,
    DROP CONSTRAINT inbox_notifications_kind_check,
    ADD CONSTRAINT inbox_notifications_kind_check CHECK (kind IN (
        'review_requested', 'review_comment', 'review_approved', 'review_rejected',
        'review_merged', 'shared_update', 'welcome', 'project_joined', 'project_removed',
        'project_role_changed', 'org_role_changed'
    ));

UPDATE inbox_notifications n SET org_id = p.org_id
FROM projects p WHERE p.project_id = n.project_id;
ALTER TABLE inbox_notifications ALTER COLUMN org_id SET NOT NULL;
