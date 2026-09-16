---
description: Start a Codex task in the bound repository and verify retrieval of the organization Memory selected by your Project.
prev:
  text: Select organization Memory
  link: /quickstart/select-memory
next:
  text: Ask Codex to update Memory
  link: /quickstart/update-memory
---

# 3. Use project Memory in Codex

You have selected the organization's deployment rollback checklist for **Payments**. Start a Codex task in the bound repository and use that knowledge in your work.

## Before you begin

- Payments can open `deployment-rollback.md`.
- The local `clumsies-demo` repository is bound to Payments.
- Codex is installed and enabled in Clumsies under **Settings → Agents**.

The Codex status should read **Plugin installed and enabled**. For installation or repair, follow [Connect a coding agent](../guides/agent-runtime). First use also requires the retrieval models and index to finish preparing.

## 1. Start from the bound repository

After the Plugin is first installed or updated, restart Codex and start a new task from `clumsies-demo`. Inspect and trust the Clumsies Hook in `/hooks` to enable activity recording.

The integration is installed once for your local user. Repository bindings determine which Project each task uses. Selecting Payments in the Clumsies window does not switch arbitrary Codex tasks to that Project.

## 2. Describe the work normally

Send this request in the new task:

> Help me prepare the pre-release checks and rollback plan for Payments. Use the project's existing guidance to list the checks and identify the Memory documents they come from. Only prepare a plan for now.

The Clumsies integration includes instructions for Codex to call `memory.activate` when starting a substantive task. It retrieves relevant passages and uses `memory.load` when full context is needed. You do not need to paste the knowledge base into the conversation or write tool arguments yourself.

“Automatic retrieval” means the agent follows those integration instructions and calls the tool. Verify the actual call; Hook activity recording does not perform retrieval for it.

## 3. Confirm that it used existing knowledge

Inspect the tool calls and the answer:

1. The task calls `activate` through Clumsies's **memory** tool.
2. Retrieved content includes the checklist selected by Payments, and the answer uses it to explain release or rollback requirements.
3. Ask Codex to read the complete checklist. Confirm that the returned path is `deployment-rollback.md` and that it contains the sentence checked on the previous page: **发布前先确认上一版可以恢复。**

An answer saying “I read the memory” is insufficient. Check the source path and returned content. The first search need not rank this document first; provide the additional cue “deployment rollback checklist” if needed, then inspect the actual result.

## If the checklist is missing

| Symptom | Check first |
| --- | --- |
| No Clumsies tool in the task | Confirm the Plugin is installed and enabled; restart Codex and start a new task. |
| The tool is available, but retrieval did not happen | Explicitly ask Codex to find deployment rollback guidance through Clumsies, then inspect the call. An ordinary answer is not retrieval evidence. |
| The tool reports an unbound repository | Start from a bound directory. A Git worktree can use the main checkout's binding; check the task's actual directory if resolution fails. |
| Retrieval is preparing | Wait for models and the index; see [troubleshooting](../guides/troubleshooting) for download or synchronization failures. |
| Retrieval works, but the checklist is absent | Confirm that Payments selects it. An organization resource is not automatically used by every Project. |

**Result:** Codex finds and uses the project's existing checklist.

Next, handle a gap discovered during work: [ask Codex to update Memory](./update-memory).
