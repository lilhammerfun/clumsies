-- Preserve explicit empty folders in resources and immutable commit trees.
ALTER TABLE resources ADD COLUMN is_directory BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE resources ADD CONSTRAINT directory_has_no_body CHECK (NOT is_directory OR body = '');
ALTER TABLE tree_entries ADD COLUMN is_directory BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE tree_entries ADD CONSTRAINT directory_is_memory CHECK (NOT is_directory OR resource_kind = 'memory');
