---
title: Reading paths
description: A first introduction to Clumsies concepts, architecture, core data structures, domain interfaces, and end-to-end flows.
---
# Start with Clumsies

Clumsies lets a team save reusable knowledge as **Memory** for coding agents to find and use during tasks. Edits become drafts; people review and publish them before the team shares a new official version.

These docs are for members, developers, and maintainers encountering the project for the first time. You do not need to read the code or know previous Clumsies versions first. [阅读中文文档](/zh/).

## Build a working understanding in about half an hour

Times are reading suggestions and exclude hands-on work. The chapters use one deployment rollback checklist example.

| Order | Read | You should be able to answer |
| --- | --- | --- |
| 1 · About 5 minutes | [Meet Clumsies](/overview) | What problem does it solve? What are Organization, Project, and Memory? |
| 2 · About 7 minutes | [System architecture](/architecture) | How do Desktop, daemon, and Server cooperate? Which data is local? |
| 3 · About 10 minutes | [Core data structures](/data-model) | What are the key fields and relationships of content, a Draft, and a Commit? |
| 4 · About 8 minutes | [End-to-end flows](/flows) | How is an edit saved, synchronized, reviewed, published, and read by an agent? |

Then open [Domain interfaces](/reference/domain-api) to connect each step to MCP, local XPC, or HTTP. Use the [Glossary](/glossary) when you encounter an unfamiliar term; you do not need to learn every term in advance.

## Read for your task

| I want to… | Start here |
| --- | --- |
| Use the product | [First use](/guides/how-to-use-clumsies) → [Agent integration](/guides/agent-runtime) |
| Understand design and data | [Architecture](/architecture) → [Data structures](/data-model) → [Memory design](/unified-memory-model) |
| Build a caller or investigate an API | [Domain interfaces](/reference/domain-api) → [MCP](/mcp) / [HTTP contracts](/reference/http-api) |
| Deploy and operate a Server | [Organization deployment](/guides/deploy-for-an-org) → [Authentication](/reference/auth) → [Troubleshooting](/guides/troubleshooting) |
| Change this project's code | [Codebase map](/repos) → [Local development](/guides/development-workflow) |
| Investigate latency or missing content | [Troubleshooting](/guides/troubleshooting) → [Retrieval and evaluation](/retrieval-evaluation) → [Performance topics](/performance/) |

## Keep three outcomes separate

- **Saved locally:** the edit has reached this device's durable storage; network failure should not erase it.
- **Synchronized:** the Server has received the proposal, which still needs review.
- **Published:** an authorized merge has advanced the Organization's official version; devices subsequently prepare their new readable views.

These three states are the starting point for understanding Clumsies data and interfaces. The sidebar's “Design details” section goes deeper. Retired designs are under “Maintenance and history”; neither is a prerequisite for getting started.

Continue with [Meet Clumsies](/overview).
