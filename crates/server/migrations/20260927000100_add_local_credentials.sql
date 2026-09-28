-- Add optional local credentials and purpose-bound, single-use account actions.
ALTER TABLE users ALTER COLUMN email DROP NOT NULL;
ALTER TABLE users ADD COLUMN username TEXT;
ALTER TABLE users ADD CONSTRAINT users_username_format CHECK (
    username IS NULL OR username ~ '^[a-z0-9][a-z0-9_.-]{2,31}$'
);
CREATE UNIQUE INDEX users_username_unique ON users (username);
CREATE TABLE password_credentials (
    user_id TEXT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    password_hash TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE action_tokens (
    token_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose IN ('invitation', 'password_reset')),
    token_hash TEXT NOT NULL UNIQUE,
    created_by TEXT REFERENCES users(user_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);
CREATE INDEX action_tokens_user ON action_tokens(user_id);
ALTER TABLE oidc_login_transactions ADD COLUMN binding_session_id TEXT REFERENCES auth_sessions(session_id) ON DELETE CASCADE;
