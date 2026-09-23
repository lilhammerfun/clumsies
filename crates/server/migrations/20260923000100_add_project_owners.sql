ALTER TABLE project_members DROP CONSTRAINT project_members_role_check;
ALTER TABLE project_members ADD CONSTRAINT project_members_role_check
    CHECK (role IN ('owner', 'admin', 'member'));

-- Prefer the recorded creator when they are still an enabled administrator.
-- Older projects without creation records use their earliest enabled administrator.
WITH owners AS (
    SELECT DISTINCT ON (m.project_id) m.project_id, m.user_id
    FROM project_members m
    JOIN users u ON u.user_id = m.user_id
    WHERE m.role = 'admin' AND u.status != 'disabled'
    ORDER BY m.project_id,
        (EXISTS (SELECT 1 FROM project_creation_requests c
                 WHERE c.project_id = m.project_id AND c.user_id = m.user_id)
         OR EXISTS (SELECT 1 FROM audit_events a
                    WHERE a.target_id = m.project_id AND a.actor_user_id = m.user_id
                      AND a.action IN ('project.created', 'admin.project_created'))) DESC,
        m.joined_at, m.user_id
)
UPDATE project_members m SET role = 'owner'
FROM owners o WHERE m.project_id = o.project_id AND m.user_id = o.user_id;

-- Recover projects whose administrators were all removed or disabled using the
-- active organization owner, who already has project-administration authority.
INSERT INTO project_members (project_id, user_id, role)
SELECT p.project_id, u.user_id, 'owner'
FROM projects p
CROSS JOIN LATERAL (
    SELECT user_id FROM users
    WHERE role = 'owner' AND status = 'active'
    ORDER BY created_at, user_id LIMIT 1
) u
WHERE NOT EXISTS (SELECT 1 FROM project_members m
                  WHERE m.project_id = p.project_id AND m.role = 'owner')
ON CONFLICT (project_id, user_id) DO UPDATE SET role = 'owner';

CREATE UNIQUE INDEX project_members_one_owner_idx
    ON project_members (project_id) WHERE role = 'owner';
