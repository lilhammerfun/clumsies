# Understand Clumsies

Clumsies gives a team a shared library of knowledge that coding agents can find and use across tasks. A Memory is a Markdown document: a deployment checklist, a coding convention, or an explanation of a system. Changes go through drafts and human review before they become the team's published version.

You do not need to understand the database or MCP to start. This page introduces the few ideas used throughout the documentation.

## Start with one document

Imagine your team maintains **Deployment rollback checklist**, at `deployment-rollback.md`.

A developer working on the Payments repository needs that checklist. The team selects it for the Payments Project. When an agent starts a deployment task, Clumsies finds the relevant passages. The agent can then read the complete document.

During the task, the developer discovers a missing verification step and asks the agent to update the checklist. The update is saved as a Draft. It can be tried in that Project before publication. An organization administrator reviews and approves the change. A successful merge creates a new published version, which reaches Projects that selected the checklist.

That is the main product loop: **find knowledge → use it → propose an improvement → review → publish**. The [complete walkthrough](/flows) follows the data at each step.

## The six ideas to learn first

| Idea | Plain-language meaning | In the example |
| --- | --- | --- |
| **Memory** | One document with a stable identity, a path, a description, and Markdown content | Deployment rollback checklist |
| **Organization** | The team and its shared, published Memory library | The company that owns the checklist |
| **Project** | A working context with members, repository bindings, and a selection of organization Memory | Payments selects the checklist and coding conventions |
| **Draft** | A proposed set of changes, carried by a Project and saved before publication | Add a rollback verification step |
| **Review** | An ordered group of one or more Drafts for discussion and an authorized decision | Review the checklist and its related runbook together |
| **Commit** | An immutable snapshot created when a change is published | The new published checklist version |

A Project's selection refers to the original Memory IDs. It does not create independent copies. Updating a selected Memory later reaches the Projects that use it.

A Memory's purpose comes from its content and path. A rule, workflow, and system explanation all use the same current content model; they are not separate publication systems.

## Understand the three views

The same checklist can appear in three forms:

| View | What you are reading | Who owns it |
| --- | --- | --- |
| **Organization Memory** | The currently published checklist | Server |
| **Project projection** | The published documents selected for Payments | Server builds it from the selection and organization content |
| **Effective Memory** | The local Project projection with that Project's pending Draft changes applied | The resident daemon on this Mac |

“Projection” means a view assembled from a larger source. “Effective” means the version currently used for local reading and retrieval.

This distinction explains an otherwise surprising behavior: an agent may read your proposed checklist step before the organization has published it. The proposal changes the bound Project's local Effective Memory. It does not publish a change to every Project.

## Know where work happens

Clumsies has three long-running components:

- **Desktop** is the macOS interface for people: sign in, choose a Project, read and edit Memory, and review changes.
- **The daemon**, named `clumsiesd`, is a background process on the same Mac. It saves Draft operations, synchronizes with Server, and runs local retrieval.
- **Server** stores shared identity, permissions, Drafts, Reviews, and published history in PostgreSQL.

An agent reaches the daemon through a small **MCP proxy**. MCP is the tool protocol used by the agent host. The proxy forwards requests; it does not own another database or search engine.

Read the [architecture](/architecture) for component diagrams and the [data model](/data-model) for object relationships.

## “Saved” has three different meanings

| Stage | What has succeeded | What has not happened yet |
| --- | --- | --- |
| **Accepted locally** | The daemon committed the Draft operation to its local SQLite database and queued synchronization | Server may not have received it |
| **Synchronized** | Server has accepted the Draft and operations | The published checklist is unchanged |
| **Published** | An authorized Review merge created an organization Commit and advanced its current-version pointer | Other Macs may still be downloading the new Project snapshot or preparing their index |

The current-version pointer is called a **Ref**. A Commit stays unchanged; a Ref moves to the next Commit.

The agent's `memory.store` success means **accepted locally**. It is not a publication acknowledgment. A member's ability to propose changes also does not grant permission to approve them.

## Choose your next page

| Your question | Read next |
| --- | --- |
| How do I use this with my repository? | [Quickstart](/quickstart/) |
| What happens from retrieval through publication? | [End-to-end flows](/flows) |
| Which components run where? | [Architecture](/architecture) |
| What are the main records and version fields? | [Data model](/data-model) |
| Which domain operations and interfaces exist? | [Domain API map](/reference/domain-api) |
| Where should I start reading the source? | [Codebase map](/repos) |

## Scope of this explanation

These pages describe the current organization-authority model. Older Project-scoped records and Context / Rule / Workflow names still appear in compatibility code and some UI paths. They should not be treated as additional ways to publish new Memory.

Detailed contracts and known implementation differences belong in the [data model](/data-model) and [interface reference](/reference/domain-api). The entry pages explain the supported workflow first.

Implementation entry points: [MCP contract](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/clumsiesd/src/agent_runtime/mcp_contract.rs), [local Draft persistence](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/clumsiesd/src/state.rs), and [Review publication](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/server/src/changes/postgres.rs).
