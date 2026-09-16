---
description: Submit the Draft changed by Codex in the App, inspect the diff, and publish it to organization Memory through human review.
prev:
  text: Ask Codex to update Memory
  link: /quickstart/update-memory
next:
  text: Continue to the guides
  link: /guides/
---

# 5. Review and publish the change

On the previous page, Codex proposed a post-rollback verification requirement for `deployment-rollback.md`. This page submits the Draft as a **Review**, lets a person inspect the diff, and publishes the change to shared organization Memory.

A Review is a Clumsies change review, similar in purpose to a code PR. Submitting it does not create a GitHub Pull Request. The current `memory` tool reads Memory and saves Draft proposals; submit, review, and merge this change in the App.

## Before you start

- Complete [Ask Codex to update Memory](/quickstart/update-memory), leaving a modified Draft for this file in Payments.
- Sign in to the App as the account that created the Draft, and wait for it to synchronize to Server. A successful Codex write means the Draft was persisted and queued for synchronization, not published.
- Arrange for an organization **owner/admin** to publish it. Being a Payments Project admin does not by itself grant authority to publish organization content.

## Check the Draft and request a Review

1. Open **Memory** and select **Payments** in the top Project filter.
2. Open `deployment-rollback.md` and choose **Diff** in **Document View** to inspect the change.
3. Confirm that the existing text remains and the new sentence is correct. The excerpts below show only the sentences to check; retain the other checklist items too.

Before:

```text
发布前先确认上一版可以恢复。
```

After:

```text
发布前先确认上一版可以恢复。
回滚后验证健康检查和关键业务请求，并记录结果。
```

The new sentence means “After rollback, verify health checks and key business requests, and record the results.”

4. Right-click the file and choose **Request Review…**. With the document open, you can also choose **Request Review** from the upper-right **Memory Actions** menu.
5. Enter `Add post-rollback verification` in **Title**. In **Description**, explain why health checks and key business requests need verification.
6. Click **Request**. On success, the App switches to **Reviews** and opens the Review.

If shared changes or conflicts appear, inspect how already-published changes combine with your Draft and confirm the intended result before continuing.

## Have a person review and publish

The reviewer opens this Review in **Reviews**, waits for its detail to load, and checks the file list, diff, and explanation. Confirm that the change adds the rollback verification requirement without unintentionally deleting or replacing the existing checklist.

When the content is ready, an organization owner/admin clicks the toolbar checkmark, whose tooltip is **Approve and merge this Review**, or uses **Review → Approve** in the macOS menu bar.

**In the current App, Approve approves and merges the Review.** Success changes its status to **Merged** and publishes the modification to organization Memory. If you lack organization publication permissions, have an authorized teammate complete this step. Saving a Draft, submitting a Review, or reading the new sentence inside Payments does not prove publication.

## Check your result

1. The Review status is **Merged**.
2. Open **Memory**, switch the filter to **Org**, and open `deployment-rollback.md`. Its published body should contain the new sentence.
3. Switch back to **Payments** and confirm that the existing selection still shows the updated checklist.
4. Wait for Payments to finish synchronizing and retrieval to become ready. Start a new Codex task from the bound `clumsies-demo` repository and ask: “Use Clumsies to read the complete `deployment-rollback.md` and quote the post-rollback verification requirement.” Inspect the actual body returned by `memory.load` and confirm that it includes **回滚后验证健康检查和关键业务请求，并记录结果。**

Verify **Merged** and the published **Org** body in the first two checks before validating Codex. Project Drafts can be retrieved before publication, so retrieving the new sentence alone does not prove it was published.

Projects selecting this organization Memory use the published shared content. Payments does not need another Add to Project operation for this update.

## Common obstacles

- **Request Review… is unavailable**: confirm you are in Payments, the file has a modified Draft, and synchronization has finished. Use **Retry Draft Sync** if it is offered.
- **Only the author can submit**: use the account that created the Draft. A reviewer opens the Review after submission.
- **Approval is unavailable**: check organization publication permissions, detail loading, and whether the Review needs reconciliation with the latest shared changes.
- **The Review is still Open or was rejected**: the change is not published. Address the feedback in the Draft; the author resubmits a rejected Review for another review.

After these checks, you have completed one cycle: select knowledge, use it with Codex, explicitly request an update, publish it through human review, and have Codex read the published result. Continue to the [guides](/guides/).
