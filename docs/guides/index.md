# Guides

Start with the outcome you want. Each guide covers an operating task; the architecture and reference pages explain the design behind it.

## New to the project?

Follow this route before reading source code:

1. [Understand Clumsies](/overview): the problem it solves and the six core concepts.
2. [Architecture](/architecture): where Desktop, daemon, Server, and agent integrations run.
3. [Data model](/data-model): how Memory, Draft, Review, and version snapshots relate.
4. [End-to-end flows](/flows): follow the deployment rollback checklist from retrieval to publication.
5. [Domain API map](/reference/domain-api): connect product operations to MCP, XPC, and HTTP.
6. [Codebase map](/repos): choose the implementation entry point for your question.

If you want to try the product first, go directly to the member workflow below and return to the design pages as needed.

## Choose a task

| I want to… | Guide | Expected result |
| --- | --- | --- |
| Use Memory in my repository and propose a change | [Member workflow](/guides/how-to-use-clumsies) | A bound repository and a Draft ready for Review |
| Deploy a Server for my team | [Organization deployment](/guides/deploy-for-an-org) | A configured installation and first owner |
| Connect an agent host | [Agent runtime](/guides/agent-runtime) | A host-managed path to the resident daemon |
| Understand agent lifecycle events | [AgentRun lifecycle](/guides/agent-run-injection) | Know which events are recorded and what they do |
| Connect DeepSeek Harness | [DSH integration](/guides/dsh-integration) | MCP registration and lifecycle forwarding |
| Develop Clumsies locally | [Development workflow](/guides/development-workflow) | An isolated worktree and Dev Instance |
| Understand local caches and Draft overlays | [Memory storage boundary](/guides/rule-store-unification) | Know which data is authoritative and which is derived |

The [archived CLI page](/guides/cli-commands) explains historical commands. It is not the current onboarding path.

## Know which role you need

Members with Project access can use its Memory, create their own Drafts, submit Reviews, and participate in allowed Review discussions. Organization owners/admins manage Projects and organization-Memory selections and authorize publication. Installing an agent integration does not grant additional Server permissions.

The [member guide](/guides/how-to-use-clumsies) shows where these roles meet in one workflow. The [domain API map](/reference/domain-api) describes the enforcement boundaries.
