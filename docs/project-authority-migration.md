# Project Memory authority cutover

Date: 2026-08-26 · Migration format: version 1

This is the historical Organization-only cutover. Its target is superseded by the accepted [Project and Organization ownership decision](/project-org-memory-ownership), implemented by additive migrations that preserve existing identities. The commands below perform the old cutover, not the new migration.

## Target model

Organization is the only active Memory authority. Project remains responsible
for repository binding, membership, Organization Memory selection, its
materialization Ref, and the visibility boundary for pre-merge Draft overlays.

A new repository-specific proposal therefore has this identity:

```text
draft.project_id = <carrying Project>
draft.resource_scope = org
```

It is not represented by changing `resources.scope` from `project` to `org`.
That would bypass Review and could collide with existing Organization identity
or paths.

## Legacy conversion

For every Project containing active Project authority or an active
Project-scoped Draft, the migration:

1. verifies the active resource bodies, current Project Ref, selected
   Organization authority, and Draft ownership;
2. materializes legacy Draft operations in daemon order
   (`created_at`, Draft ID, operation ordinal), including provisional IDs;
3. combines that result with the selected Organization authority and the
   carrying user's active Organization Drafts; an exact legacy path becomes an
   update proposal for the selected resource, while an already-active Org Draft
   for that resource supersedes the legacy duplicate and is reported;
4. rejects every remaining case, prefix, or file/directory path collision;
5. records a deterministic plan hash and an Effective Memory path/content hash;
6. on apply, discards the old Drafts, archives Project resources, creates one
   Organization-scoped create/update Draft per retained final legacy file, and
   advances a Project Ref containing selected Organization Memory only.

Descriptions from the active authority row are preserved when an old Project
Commit differs only in description. Any identity, path, or body drift is a
blocker. Projects with zero or multiple active members are also blocked because
replacement Draft ownership would be ambiguous.

The apply phase is one serializable PostgreSQL transaction. It takes migration
and Organization coordination locks, rechecks Ref and Draft versions, rejects a
stale plan hash, writes audit history, and validates the permanent database
guards that prohibit active Project resources and active Project Drafts.

## Runbook

First take a PostgreSQL custom-format backup and preserve the affected clients'
SQLite databases. Exercise both commands against a restored database before
touching production.

```bash
DATABASE_URL=postgres://... clumsies-server migrate-project-authority --dry-run
```

Dry-run rolls back its planning transaction and does not run schema migrations.
Review `ready`, `blockers`, each Project's counts and warnings, and preserve the
returned `plan_hash`.

```bash
DATABASE_URL=postgres://... clumsies-server migrate-project-authority \
  --apply --expected-plan-hash 'sha256:...'
```

Apply first installs pending schema migrations, then proceeds only if the live
plan still has the exact reviewed hash. A failed apply rolls back all data
changes. Running dry-run again after success must report zero legacy authority,
zero active legacy Drafts, and zero replacements.

Verify these database invariants after apply:

```sql
SELECT count(*) FROM resources
WHERE scope = 'project' AND status = 'active';

SELECT count(*) FROM drafts
WHERE resource_scope = 'project' AND status IN ('open', 'submitted');

SELECT conname, convalidated
FROM pg_constraint
WHERE conname IN (
  'resources_no_active_project_authority',
  'drafts_no_active_project_authority'
);
```

Both counts must be zero and both constraints must be validated. Keep the
backup until clients have synchronized the discard events, replacement Drafts,
and new Project Ref. The Desktop's legacy lock accessory then disappears
naturally because no active Project-scoped rows remain; parsing and discard of
historical rows stay available only for recovery compatibility.
