-- Capture labels at insertion, including authentication and maintenance writers.
-- Existing rows intentionally remain without snapshots: their historical labels are unknown.
ALTER TABLE audit_events ADD COLUMN actor_label TEXT;
ALTER TABLE audit_events ADD COLUMN actor_email_snapshot TEXT;
ALTER TABLE audit_events ADD COLUMN target_label TEXT;
ALTER TABLE audit_events ADD COLUMN labels_recorded BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE audit_events ADD COLUMN changes JSONB NOT NULL DEFAULT '[]'::jsonb;

CREATE FUNCTION capture_audit_labels() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    SELECT COALESCE(NULLIF(display_name, ''), username, email, user_id), email
      INTO NEW.actor_label, NEW.actor_email_snapshot FROM users WHERE user_id = NEW.actor_user_id;
    CASE NEW.target_type
      WHEN 'org' THEN
        SELECT name INTO NEW.target_label FROM orgs WHERE org_id = NEW.target_id AND org_id = NEW.org_id;
      WHEN 'user' THEN
        SELECT COALESCE(NULLIF(display_name, ''), username, email, user_id)
          INTO NEW.target_label FROM users WHERE user_id = NEW.target_id;
      WHEN 'project' THEN
        SELECT name INTO NEW.target_label FROM projects WHERE project_id = NEW.target_id AND org_id = NEW.org_id;
      WHEN 'project_member' THEN
        SELECT p.name || ' · ' || COALESCE(NULLIF(u.display_name, ''), u.username, u.email, u.user_id)
          INTO NEW.target_label FROM projects p, users u
          WHERE p.project_id = split_part(NEW.target_id, ':', 1) AND p.org_id = NEW.org_id
            AND u.user_id = split_part(NEW.target_id, ':', 2);
      WHEN 'session' THEN
        SELECT COALESCE(NULLIF(u.display_name, ''), u.username, u.email, u.user_id)
          INTO NEW.target_label FROM auth_sessions s JOIN users u USING (user_id)
          WHERE s.session_id = NEW.target_id AND s.org_id = NEW.org_id;
      WHEN 'access_token' THEN
        SELECT COALESCE(NULLIF(u.display_name, ''), u.username, u.email, u.user_id)
          INTO NEW.target_label FROM access_tokens t JOIN users u USING (user_id)
          JOIN auth_sessions s USING (session_id)
          WHERE t.token_id = NEW.target_id AND s.org_id = NEW.org_id;
      WHEN 'action_token' THEN
        SELECT COALESCE(NULLIF(u.display_name, ''), u.username, u.email, u.user_id)
          INTO NEW.target_label FROM action_tokens t JOIN users u USING (user_id)
          WHERE t.token_id = NEW.target_id;
      ELSE NULL;
    END CASE;
    NEW.labels_recorded := TRUE;
    RETURN NEW;
END;
$$;
CREATE TRIGGER audit_labels BEFORE INSERT ON audit_events
FOR EACH ROW EXECUTE FUNCTION capture_audit_labels();
