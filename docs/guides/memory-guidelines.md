---
description: Choose how agents organize, update, and retire knowledge with an editable Memory Guidelines document.
---

# Memory Guidelines

Memory guidelines tell agents what is worth remembering and how to organize, update, and retire knowledge in your memory space. They live in an editable Memory document named `CLUMSIES.md` by default.

Start with the Clumsies defaults or use your team's existing guidelines. You can edit the document and share it through your organization's normal review process. App updates preserve your changes.

## Start with the defaults

1. Open **Memory** and select an empty Project.
2. Choose **Preview guidelines and their sources** to read the complete English template and its research references, or choose **Use Default Guidelines** to adopt it directly.
3. Clumsies creates `CLUMSIES.md` as a Draft and opens it. Use **Source** to edit it whenever your conventions change.

The Draft contributes to this Project's Effective Memory before publication. To share it with other Projects, use the existing [review and publication flow](/quickstart/review-and-publish), then select the published resource in those Projects.

You can also choose **Create a Memory** or **File → New Memory** to start with your own content. Adopting default guidelines is optional.

## Use existing guidelines

Clumsies checks the configured path, current Project drafts, and Organization Memory before offering initialization. Existing guidelines are preserved. If the Organization already has the document, **Use Organization Guidelines** adds that resource to the Project's selection; this requires Project administrator access. It does not create another copy.

A missing custom path is reported so you can restore the document or correct the configuration. It does not silently create the defaults at another path. A Draft that removes or renames the configured document must be resolved first.

The path is a location inside Clumsies Memory, not a file in your repository or plugin cache. The current custom-path setting belongs to the local daemon; the App does not yet offer independent per-Project path settings.

## What the defaults cover

- Keep decisions, constraints, verified procedures, and lessons that matter to future work.
- Preserve your existing layout; new spaces can use `knowledge/`, `procedures/`, and `lessons/` as needed.
- State applicability and keep reasons, evidence, and exceptions alongside the guidance they qualify.
- Update the existing document for the same topic; create a new document for an independently maintainable topic.
- Resolve conflicting claims using scope and evidence, and remove obsolete instructions from current guidance.

The document is ordinary Memory. It does not grant new permissions, authorize automatic writes, or install a host skill. The [Memory tool](/mcp) continues to govern operations and Draft status.

## Why these guidelines?

The defaults adapt published research and engineering experience to Clumsies:

| Design choice | Source |
|---|---|
| Keep relevant information with enough context for correct action | [Anthropic: Effective context engineering for AI agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents) |
| Check existing notes and consolidate knowledge instead of continually appending | [LangChain: How we built Agent Builder's memory system](https://www.langchain.com/blog/how-we-built-agent-builders-memory-system) |
| Prefer incremental changes that preserve specific knowledge | [Agentic Context Engineering, v3](https://arxiv.org/abs/2510.04618v3) |

These sources inform the design choices; they do not validate this template's effectiveness. Adapt the defaults to your team's actual tasks and check whether updates preserve useful knowledge and reduce duplication.
