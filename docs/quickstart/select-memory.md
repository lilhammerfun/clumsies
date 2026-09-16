---
description: Select the existing deployment rollback checklist from Org so Payments can use this shared knowledge.
prev:
  text: Create a Project and bind a repository
  link: /quickstart/create-project
next:
  text: Use Memory with Codex
  link: /quickstart/use-with-agent
---

# 2. Select existing Memory

The organization's **Org** view contains published, shared Memory. Each Project selects the content relevant to its work. Project members and Codex working from a bound repository can then use that content through the Project.

This example selects the existing `deployment-rollback.md` for **Payments**. Each Project can use the same organization checklist.

## Before you start

- Complete [Create a Project and bind a repository](/quickstart/create-project), and confirm Payments is available in the App.
- Your organization has published `deployment-rollback.md`, containing **发布前先确认上一版可以恢复。** This means “Before deployment, confirm that the previous version can be restored.”
- You are a **Project admin** for Payments or an organization **owner/admin**. Ordinary Project members can use content already selected by an administrator; ask an administrator to add any missing selection.

If the organization has no content yet, ask an administrator to prepare and publish the checklist using [Create new organization Memory](/guides/create-memory), then return here. Preparing the initial knowledge is a separate task, not a requirement for every new member.

## Select the checklist from Org

1. Open **Memory** in the sidebar. If Project Settings is still open, click its gear button to return to content.
2. Open the top Project filter and select **Org**.
3. Find and open `deployment-rollback.md` in the file tree. Confirm that its body contains **发布前先确认上一版可以恢复。**
4. Right-click the file and choose **Add to Project → Add to Payments**.
5. Wait for the operation to finish, then switch the top filter to **Payments**.

The action is called **Add to Project**. It changes the list of organization Memory selected by the Project. The file remains the same shared resource; selection does not edit its body or require a Review.

## Check your result

Open `deployment-rollback.md` from the **Payments** file tree. You should be able to read the same sentence. The original file remains visible in **Org** too.

The Project now has knowledge to use. Next, you will start Codex from the bound `clumsies-demo` repository and verify that it can retrieve this Memory while handling a task.

## Common obstacles

- **The file is missing from Org**: check your organization and search filters. The file may not have been published; Project Drafts do not appear as published Org files.
- **Add to Payments is disabled**: the action requires a Payments Project admin or organization owner/admin, and must wait for any blocking document synchronization.
- **Payments is missing from the menu**: confirm that you have access to the Project. Its administrator can add you; if it has not been created, return to the previous step.
- **The file is still missing from Payments**: check for an error from the selection action and confirm the filter shows Payments. Retry any failed loading before proceeding; do not create a duplicate file to work around an incomplete selection.

Next: [Use Memory with Codex](/quickstart/use-with-agent).
