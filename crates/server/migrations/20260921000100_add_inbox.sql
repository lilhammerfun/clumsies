-- Keep one personal notification per subject; receipts acknowledge only the version seen.
CREATE TABLE inbox_notifications (
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    notification_id TEXT NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('review_requested', 'review_comment', 'review_approved', 'review_rejected', 'review_merged', 'shared_update')),
    target_id TEXT NOT NULL,
    actor_user_id TEXT REFERENCES users(user_id) ON DELETE SET NULL,
    event_key TEXT NOT NULL,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    read_version BIGINT NOT NULL DEFAULT 0 CHECK (read_version >= 0 AND read_version <= version),
    archived_version BIGINT NOT NULL DEFAULT 0 CHECK (archived_version >= 0 AND archived_version <= version),
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, notification_id)
);

-- Existing open reviews must remain discoverable when their file-tree reminders disappear.
INSERT INTO inbox_notifications (user_id, notification_id, project_id, kind, target_id, actor_user_id, event_key, occurred_at)
SELECT m.user_id, 'review:' || r.review_id, r.project_id, 'review_requested', r.review_id,
       r.author_user_id, 'review_requested:' || r.version::text, r.updated_at
FROM reviews r
JOIN project_members m ON m.project_id = r.project_id
JOIN users u ON u.user_id = m.user_id
WHERE r.status = 'open' AND u.role IN ('owner', 'admin') AND u.status = 'active'
  AND m.user_id <> r.author_user_id;
