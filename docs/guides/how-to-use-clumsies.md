# Use Clumsies with your repository

This is a task index for people returning to an existing workspace. Choose the section you need; it is not another sequence to follow from top to bottom. New readers should start with the [quickstart](/quickstart/).

## Sign in to your organization {#_1-sign-in-to-your-organization}

Follow [Connect to your organization](/quickstart/connect) to sign in and confirm the intended organization. Account admission and credentials are explained in [Authentication and sessions](/reference/auth).

## Choose a Project and attach the repository {#_2-choose-a-project-and-attach-the-repository}

Use [Create a project](/quickstart/create-project) for the Project and repository binding steps. When your team has already assigned a Project, reuse it. [Project selection and binding](/workspace) explains how the shared Project differs from a local directory.

## Check the Memory selection {#_3-check-the-memory-selection}

Use [Select Memory](/quickstart/select-memory) when the organization has a document that your Project needs. A Project admin or organization owner/admin can change that selection. If the document does not exist yet, use [Create a new Memory](/guides/create-memory).

## Connect your agent host {#_4-connect-your-agent-host}

For normal Codex use, follow [Use Memory with Codex](/quickstart/use-with-agent). To configure or troubleshoot a host connection, use [Agent integration](/guides/agent-runtime).

## Make one proposed improvement {#_5-make-one-proposed-improvement}

Follow [Ask Codex to propose an update](/quickstart/update-memory). Explicitly authorize maintaining the Memory and inspect the resulting Draft. Saving or synchronizing the proposal does not publish it; [End-to-end flows](/flows) explains the state boundaries.

## Resolve shared updates and request Review {#_6-resolve-shared-updates-and-request-review}

Use [Review and publish](/quickstart/review-and-publish) to inspect the current proposal and submit it for human review. If shared content changed or the request failed, use [Troubleshooting](/guides/troubleshooting) before creating another proposal.

## Have an authorized reviewer publish it {#_7-have-an-authorized-reviewer-publish-it}

An organization owner/admin completes publication. [Review and publish](/quickstart/review-and-publish) covers the decision and the return to Codex to verify the shared content.

## Choose the app language

In **Clumsies → Settings → General → Language**, choose **Follow System**,
**English**, or **简体中文**. This changes only Clumsies: you can keep your Mac in
English and use Clumsies in Chinese. Choose **Restart and Apply** to save pending
changes and reopen the same App automatically with the selected language. If you
keep editing or a save fails, the restart is cancelled. **Later** keeps the
current session open and applies the choice on the next launch.

## When something does not work

Start with [Troubleshooting](/guides/troubleshooting), which groups problems by visible symptoms. For a retrieval or integration failure, also check [Agent integration](/guides/agent-runtime).

## Local storage and administration

For cache recovery and preserved edits, read [Troubleshooting](/guides/troubleshooting). For local data ownership, see [Runtime](/runtime). Organization setup and access belong in [Deployment](/guides/deploy-for-an-org) and [Authentication](/reference/auth).

## Implementation references

Use [Domain interfaces](/reference/domain-api) to locate the boundary involved, and [Codebase map](/repos) to find the implementation. [Architecture](/architecture) and [Data model](/data-model) explain the relationships.
