# Organization Memory

Organization Memory is the team's published knowledge library. Several Projects can select the same Memory, while its identity and publication history are maintained once. The [core data model](/data-model) follows one document through proposal and Review publication.

This page retains the historical `/artifact` URL. Artifact and Hub are retired product names, not additional objects or services in the current system.

## Organization owns content; Projects choose what to use

Suppose an Organization has coding conventions, payment API notes, and a deployment rollback checklist. Payments may select all three; Website may select the conventions and checklist. Both Projects reference the same Memory IDs instead of maintaining independent copies of the bodies.

The Organization Ref identifies the published Organization Commit. Each Project's selection produces its own Project Commit. When selected upstream resources change, Server refreshes affected projections and local daemons synchronize them.

| Action | What changes |
|---|---|
| Select / remove Memory in a Project | The Project's selection and projection |
| Edit Memory | First creates an Organization Draft carried by that Project |
| Submit Review | Presents one or more Drafts for human coordination |
| Merge Review | Updates Organization publication and affected Project projections |
| Rename Memory | Changes its published path; stable ID remains |
| Delete Memory | Archives the current resource at publication; immutable snapshots retain history |

Ordinary members propose, submit, and comment within their permissions. Organization owners/admins hold publication decision permissions. Agent `memory.store` only saves proposals; MCP exposes no approval or merge tool. See [API reference](/reference/) for endpoint authorization.

## Contents of a Memory

Each Memory has a stable ID, Organization-relative path, path-derived `name`, `description`, Markdown body, and status. See the [Memory field table](/data-model#memory-identity-path-and-content) for fields and nullability.

Bodies can describe rules, procedures, or background knowledge, but these are not three system content types. Paths and headings organize knowledge for people; they grant no additional permission and do not automatically turn documents into executable host Skills.

Descriptions can currently be empty, and Server merge does not fully persist Draft descriptions. A description can help explain a resource, but clients cannot assume every published Memory has a reliable one. See [implementation boundaries](/unified-memory-model#current-implementation-boundaries).

## Bundle: a personal collection of Memory

A Bundle is a user's Server-stored set of Memory IDs for grouping, discovery, and reuse. A “New teammate onboarding” Bundle might contain coding conventions and the deployment rollback checklist.

- Memory can belong to no Bundle or several Bundles.
- Editing a Bundle changes the collection, not content, identity, or publication state.
- Saving Memory in a Bundle does not automatically add it to a Project Org Selection.
- Deleting a Bundle does not delete its Memory.

The tables are `personal_bundles` and `personal_bundle_items`. Project selection uses a separate pair of tables; see the [storage mapping](/data-model#where-the-data-lives).

Continue with [Project](/workspace) and [Unified Memory design](/unified-memory-model). Implementation sources: [Memory API](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/memory/dto.rs) and [Memory persistence](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/memory/repository.rs), [Bundle persistence](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/bundle/repository.rs).
