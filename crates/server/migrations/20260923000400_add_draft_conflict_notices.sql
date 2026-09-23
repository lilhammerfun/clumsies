ALTER TABLE inbox_notifications DROP CONSTRAINT inbox_notifications_kind_check;
ALTER TABLE inbox_notifications ADD CONSTRAINT inbox_notifications_kind_check CHECK (kind IN (
    'review_requested', 'review_comment', 'review_approved', 'review_rejected', 'review_merged',
    'shared_update', 'welcome', 'project_joined', 'project_removed', 'project_role_changed',
    'org_role_changed', 'draft_conflict'
));
