CREATE TABLE project_episode_states (
    project_id TEXT PRIMARY KEY REFERENCES projects(project_id) ON DELETE CASCADE,
    corpus_revision BIGINT NOT NULL DEFAULT 0 CHECK (corpus_revision >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE project_episode_summary_policies (
    project_id TEXT PRIMARY KEY REFERENCES projects(project_id) ON DELETE CASCADE,
    instructions TEXT NOT NULL DEFAULT '',
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision >= 1),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (octet_length(instructions) <= 8000)
);

CREATE TABLE project_episodes (
    episode_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    run_id TEXT NOT NULL,
    host_session_id TEXT,
    host TEXT NOT NULL,
    evidence_format TEXT NOT NULL,
    evidence_format_revision BIGINT NOT NULL CHECK (evidence_format_revision >= 1),
    activity_at TIMESTAMPTZ NOT NULL,
    evidence_hash TEXT NOT NULL CHECK (evidence_hash ~ '^sha256:[0-9a-f]{64}$'),
    evidence_bytes BIGINT NOT NULL CHECK (evidence_bytes >= 0),
    status TEXT NOT NULL CHECK (status IN ('pending_summary', 'active', 'no_memory', 'deleted')),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision >= 1),
    corpus_revision BIGINT NOT NULL CHECK (corpus_revision >= 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    UNIQUE (project_id, run_id),
    UNIQUE (project_id, corpus_revision),
    CHECK (length(trim(run_id)) BETWEEN 1 AND 200),
    CHECK (
        host_session_id IS NULL OR (
            host_session_id = trim(host_session_id)
            AND octet_length(host_session_id) BETWEEN 1 AND 256
        )
    ),
    CHECK (length(trim(host)) BETWEEN 1 AND 100),
    CHECK (length(trim(evidence_format)) BETWEEN 1 AND 100),
    CHECK ((status = 'deleted') = (deleted_at IS NOT NULL))
);

CREATE INDEX project_episodes_project_activity_idx
    ON project_episodes(project_id, activity_at DESC, episode_id);

CREATE TABLE project_episode_evidence (
    episode_id TEXT NOT NULL REFERENCES project_episodes(episode_id) ON DELETE CASCADE,
    sequence BIGINT NOT NULL CHECK (sequence >= 0),
    occurred_at TIMESTAMPTZ NOT NULL,
    kind TEXT NOT NULL CHECK (length(trim(kind)) BETWEEN 1 AND 100),
    content TEXT NOT NULL,
    PRIMARY KEY (episode_id, sequence)
);

CREATE TABLE project_episode_summaries (
    episode_id TEXT NOT NULL REFERENCES project_episodes(episode_id) ON DELETE CASCADE,
    revision BIGINT NOT NULL CHECK (revision >= 1),
    evidence_hash TEXT NOT NULL CHECK (evidence_hash ~ '^sha256:[0-9a-f]{64}$'),
    summary_algorithm_revision TEXT NOT NULL,
    policy_revision BIGINT NOT NULL CHECK (policy_revision >= 1),
    body TEXT NOT NULL,
    no_memory BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (episode_id, revision),
    CHECK (length(trim(summary_algorithm_revision)) BETWEEN 1 AND 100),
    CHECK ((no_memory AND body = '') OR (NOT no_memory AND length(trim(body)) > 0))
);

CREATE TABLE project_episode_ingest_requests (
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL CHECK (request_hash ~ '^sha256:[0-9a-f]{64}$'),
    episode_id TEXT NOT NULL REFERENCES project_episodes(episode_id) ON DELETE CASCADE
        DEFERRABLE INITIALLY DEFERRED,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, user_id, idempotency_key),
    CHECK (length(idempotency_key) BETWEEN 1 AND 200)
);

CREATE FUNCTION reject_project_episode_evidence_update()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'Project Episode Evidence is immutable';
END;
$$;

CREATE TRIGGER project_episode_evidence_immutable
BEFORE UPDATE ON project_episode_evidence
FOR EACH ROW EXECUTE FUNCTION reject_project_episode_evidence_update();

INSERT INTO project_episode_states (project_id)
SELECT project_id FROM projects;

INSERT INTO project_episode_summary_policies (project_id)
SELECT project_id FROM projects;
