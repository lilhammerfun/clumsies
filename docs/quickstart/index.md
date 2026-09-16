---
title: Quickstart
description: Select team knowledge, use it with Codex, propose an update, and publish it through human review.
prev: false
next:
  text: Create a project
  link: /quickstart/create-project
---
# Use and update team Memory with Codex

This tutorial follows an existing **Deployment rollback checklist** at `deployment-rollback.md`. You will select it for a **Payments** project, let Codex use it in `clumsies-demo`, ask Codex to propose an improvement, and publish the proposal through human review.

The final check happens back in Codex: read the Memory again and confirm that the published change is present.

## Before you start

**WIP:** check [Get the App](/quickstart/install) for current availability, then [Connect to your organization](/quickstart/connect). Installation and sign-in come before the five tutorial steps.

You need a signed-in organization account, a practice repository named `clumsies-demo`, and Codex. Project creation covers repository binding; the Codex page explains how the managed integration supplies Memory to a task.

The organization must already have a published `deployment-rollback.md`. It is an example used by these docs, **not built-in sample content**. If the organization has no suitable Memory, complete [step 1: create a Project](/quickstart/create-project), then follow [Create and publish a Memory](/guides/create-memory) with an authorized reviewer before continuing to step 2. Selecting an existing Memory adds its reference to the Project; it does not make a separate editable copy.

## Who does each part?

| Operation | Required role |
| --- | --- |
| Create a Project | An organization member; the creator becomes its Project admin |
| Select organization Memory for the Project | A Project admin or an organization owner/admin |
| Publish a Review | An organization owner/admin |

If another person will publish the Review, arrange that handoff before the final step. Working in Codex does not grant additional organization permissions.

## Follow these five steps

| Step | What you do | Result to check |
| --- | --- | --- |
| 1. [Create a project](/quickstart/create-project) | Create Payments and bind `clumsies-demo` | The repository is associated with the intended Project |
| 2. [Select Memory](/quickstart/select-memory) | Select the existing organization checklist | The Project includes the shared document |
| 3. [Use it with Codex](/quickstart/use-with-agent) | Start a task that needs the checklist | Codex retrieves and reads the relevant Memory |
| 4. [Ask Codex to propose an update](/quickstart/update-memory) | Explicitly request a change to that Memory | A Draft contains the proposed edit; the organization version is unchanged |
| 5. [Review and publish](/quickstart/review-and-publish) | A person submits Review, an authorized reviewer publishes, then Codex reads again | The shared version contains the change and Codex can retrieve it |

Codex handles retrieval and reading as part of the task. You do not need to write MCP requests to follow this tutorial. A normal coding request also does not authorize changing team Memory; step 4 makes that instruction explicit.

`deployment-rollback.md` is a Memory path, not a file to copy into the practice repository. Keep the same Project, repository, and Memory throughout the tutorial.

## What you should understand afterward

You should be able to explain why selecting Memory does not copy it, why a saved Draft is not yet a shared update, and why publication requires a person's Review decision. For the design behind these actions, continue with [End-to-end flows](/flows) and [Core data structures](/data-model).

Begin with [Create a project](/quickstart/create-project). For a single recurring task, use [Task guides](/guides/).
