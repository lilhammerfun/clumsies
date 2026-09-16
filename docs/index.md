---
title: Start with Clumsies
description: Find a reading route for using Clumsies, understanding its design, or developing and operating it.
---
# Start with Clumsies

Clumsies gives a team a shared collection of **Memory**: reusable knowledge that coding agents can find and use. A Project selects the knowledge relevant to its work. Proposed edits become Drafts, and people review and publish them before the shared version changes. [阅读中文文档](/zh/).

Choose a route below. You can learn the design without installing the App, or try the workflow and return to the explanations as questions arise.

## Use Clumsies with your team

Start with the [quickstart](/quickstart/). Create a project, select an existing organization Memory, let Codex use it, explicitly request a change, and review and publish that change.

**Result:** Codex uses the knowledge selected for your project, and you can tell a saved proposal from a published team update. The tutorial follows Payments, the practice repository `clumsies-demo`, and `deployment-rollback.md`.

Already working in a project? Open [Task guides](/guides/) to select just the task you need.

## Understand the design

This route is for a reader who wants to understand how the project is built. Begin with [Meet Clumsies](/overview), then follow:

| Read | Question it answers |
| --- | --- |
| [System architecture](/architecture) | What do the App, local daemon, Server, and agent integration each own? |
| [Core data structures](/data-model) | How are Memory, Draft, Review, and versions represented and connected? |
| [End-to-end flows](/flows) | How does data move through retrieval, local edits, synchronization, and publication? |
| [Domain interfaces](/reference/domain-api) | Which boundary does an operation cross, and which interface handles it? |

**Result:** You can explain where data lives, which version a reader sees, and why saving, synchronizing, and publishing are different outcomes. Use the [Glossary](/glossary) for unfamiliar terms; subsystem details remain available in the sidebar.

## Integrate, operate, or develop Clumsies

Choose the part of the system you are responsible for:

| Your work | Reading route | Expected result |
| --- | --- | --- |
| Build an integration | [Domain interfaces](/reference/domain-api) → [MCP](/mcp) or [HTTP](/reference/http-api) | Find the right operation, inputs, permissions, and error handling |
| Operate a team service | [Deployment](/guides/deploy-for-an-org) → [Authentication](/reference/auth) → [Troubleshooting](/guides/troubleshooting) | Configure access and locate the cause of an operational problem |
| Change the implementation | [Codebase map](/repos) → [Development workflow](/guides/development-workflow) | Find the relevant code and run an isolated development instance |

The [reference index](/reference/) is for looking up contracts. The [task guides](/guides/) are for completing a specific job. Historical designs and dated performance evidence are under Development and maintenance.

## Before using the App

**WIP:** the native App is still in development. Check [Get the App](/quickstart/install) for current availability, then [Connect to your organization](/quickstart/connect). Installation and sign-in are preparation for the tutorial; they are not required for reading the design documentation.
