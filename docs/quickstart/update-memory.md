---
description: Explicitly ask Codex to update existing Memory, inspect the saved Draft, and prepare for human review.
prev:
  text: Use project Memory in Codex
  link: /quickstart/use-with-agent
next:
  text: Review and publish
  link: /quickstart/review-and-publish
---

# 4. Ask Codex to update Memory

Codex has read the deployment rollback checklist. Suppose you notice that it explains restoring the previous version but does not require checking the result. Save that discovery as one proposed change.

## Before you begin

Continue in the Codex task bound to **Payments**. You need access to the Project and your own Clumsies account to propose a Draft.

First confirm that the checklist does not already contain the verification step below. If your real document already covers it, choose another gap that actually needs attention.

## 1. Explicitly request a knowledge update

Give Codex a specific request:

> Update `deployment-rollback.md` in Clumsies. First load the full document and its applicable Memory maintenance rules. Add this requirement to the rollback guidance: “回滚后验证健康检查和关键业务请求，并记录结果。” Preserve the existing content, save the change as a Draft for review, and report the change summary and save result. Do not change repository files in this task.

The new sentence means: verify health checks and critical business requests after rollback, then record the results. Keeping this exact example text makes the proposed change easy to compare across the tutorial.

An ordinary coding task, feedback in a conversation, or “remember this next time” is not necessarily a request to maintain Memory. The current integration requires an explicit maintenance request before the agent calls `memory.store`.

This updates Clumsies knowledge. It does not automatically change a repository file or create a GitHub Pull Request.

## 2. Inspect the save result

Codex should first use `load` to read the complete document, then `store` to save exact replacements. Loading the source lets it preserve surrounding content and check the version it is editing.

Check the tool result and summary:

- The target is the same `deployment-rollback.md`.
- The added requirement covers verification after rollback; existing checks remain.
- The result identifies a Draft or local operation. “Saved” is not reported as “published to the organization.”

If version validation fails, ask Codex to reload the current content and inspect the differences before editing again. Do not bypass conflicts or overwrite someone else's changes. Field details belong in the [MCP reference](../mcp); this tutorial does not require you to enter hashes or resource IDs yourself.

## 3. Inspect the Draft in the App

Return to Clumsies, open **Memory**, select **Payments**, and open **Diff** for `deployment-rollback.md`. The new verification sentence should be visible, with the surrounding content preserved.

The daemon synchronizes the Draft to the Server in the background. Agents in the current Project can read the proposal before publication, while the organization's published version remains unchanged.

| Current result | What it proves |
| --- | --- |
| `store` succeeds | The change is durable locally and queued for synchronization. |
| The Draft is synchronized and **Request Review…** is available | You can submit your change for review. |
| The current Project reads the new sentence | The local proposal is effective; this alone does not prove organization publication. |

**Result:** Payments has an inspected change waiting for review.

Next: [submit a Review, inspect it, and publish](./review-and-publish).
