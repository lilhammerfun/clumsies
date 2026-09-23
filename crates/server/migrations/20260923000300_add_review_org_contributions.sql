-- A contribution is an intent plus a link to a normal independent Review.
CREATE TABLE review_org_contributions (
    source_review_id TEXT PRIMARY KEY REFERENCES reviews(review_id),
    entries JSONB NOT NULL CHECK (jsonb_typeof(entries) = 'array' AND jsonb_array_length(entries) > 0),
    source_commit_id TEXT REFERENCES commits(commit_id),
    org_review_id TEXT UNIQUE REFERENCES reviews(review_id),
    last_error TEXT,
    CHECK (org_review_id IS NULL OR source_commit_id IS NOT NULL)
);
