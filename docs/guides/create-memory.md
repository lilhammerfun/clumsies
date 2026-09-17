---
description: Prepare new organization Memory when no suitable content exists, then publish the Draft through human review for Projects to select.
prev:
  text: Guides
  link: /guides/
next:
  text: Select existing Memory
  link: /quickstart/select-memory
---

# Create new organization Memory

When your organization has no suitable knowledge yet, create and publish Memory for Projects to select. This guide is for organization administrators preparing initial content and Project members proposing new knowledge.

If `deployment-rollback.md` already exists, go directly to [Select existing Memory](/quickstart/select-memory). Everyday use normally starts with the team's existing knowledge.

## Before you start

Sign in to the App and ensure you can access a Project such as **Payments**. If you have no Project yet, first [create one](/quickstart/create-project), then return here. Project members can create Drafts; publishing to the organization requires an **owner/admin**. If you are an ordinary member, arrange for an authorized teammate to review your proposal.

You are creating Clumsies Memory; this does not create a file in the `clumsies-demo` repository. The current App has no Memory import button corresponding to **Export as ZIP…**. For a small amount of new content, use the editing workflow below.

## Create and write the Draft

1. Open **Memory** and select **Payments** in the top Project filter. If Project Settings is still open, click its gear button to return to content.
2. Choose **File → New Memory** in the macOS menu bar, or press **⌘N**. The empty list's **Create a Memory** button also creates a Draft. To start with a document that tells agents how to maintain knowledge, choose **Use Default Guidelines** instead; see [Memory Guidelines](/guides/memory-guidelines).
3. A Draft with a default filename, usually `untitled.md`, appears in the file tree. Right-click it and choose **Rename…**.
4. In **Rename Draft**, enter `deployment-rollback.md` in **File name** and click **Rename**. This example uses a root-level filename; do not include `/`.
5. Right-click the file and choose **Open Source**, or select **Source** in **Document View** for an open file. Replace the body with:

```markdown
# Deployment rollback checklist

发布前先确认上一版可以恢复。

- Before deployment: confirm automated checks pass; record the current version and rollback steps.
- After deployment: check key pages and error rates; restore the previous version and record the cause if something goes wrong.
```

Keep the Chinese sentence as the tutorial's verification marker. It means “Before deployment, confirm that the previous version can be restored.”

The App saves automatically after you stop typing and synchronizes the Draft in the background. Use **Preview** to check formatting or **Diff** to inspect the proposed content.

## Submit and publish

1. Wait for synchronization to finish, then right-click the file and choose **Request Review…**.
2. Set **Title** to `Add deployment rollback checklist`, add a **Description** if needed, and click **Request**. Submit using the account that created the Draft.
3. In **Reviews**, inspect the complete new file. An organization owner/admin then clicks the checkmark with the tooltip **Approve and merge this Review**, or chooses **Review → Approve**.
4. Wait for the Review status to become **Merged**.

See [Review and publish the change](/quickstart/review-and-publish) for more detail about reviewing. This guide adds a new file; the main quickstart example updates an existing one.

## Check your result

Switch Memory's top filter to **Org**. You should find `deployment-rollback.md` and read **发布前先确认上一版可以恢复。** in its body. Seeing the text only in a Payments Draft does not prove publication.

## Common obstacles

- **New Memory is unavailable**: select a Project first. Projects carry proposals; the Org view displays published content.
- **Request Review… is unavailable**: wait for Draft synchronization, and resolve any synchronization error first.
- **You cannot publish**: ask an organization owner/admin to review and publish. Being a Project admin does not grant that permission.
- **The checklist already exists**: use the existing Memory. Follow [Ask Codex to update Memory](/quickstart/update-memory) if it needs additional information.

Return to [Select existing Memory](/quickstart/select-memory) so Projects can select the prepared knowledge.
