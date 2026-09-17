# Memory Guidelines

This document tells agents what is worth remembering and how to organize, update, and retire knowledge in this memory space.

Keep memory useful for future work: specific, scoped, supported, and easy to update. Apply these defaults when the user requests memory maintenance. Follow explicit user instructions and established project conventions. This document does not authorize additional writes.

## What to keep

Keep durable knowledge that would change a future answer or action: decisions and their reasons, project constraints, verified procedures, and lessons that prevent a known failure.

Unless the user requests a particular record, leave out temporary progress, raw conversations, large logs, unsupported guesses, and information already easy to recover from the maintained source. Link to that source and retain the non-obvious context. Never include credentials or secrets.

## Where it belongs

Preserve the existing organization. In a new space, use these paths as needed:

- `knowledge/<topic>.md` for facts, constraints, and decisions.
- `procedures/<task>.md` for repeatable steps and their verification.
- `lessons/<failure>.md` for a verified failure, its cause, and prevention.

Create only the documents needed. Use one canonical document per independently maintainable topic. Link related documents instead of copying their rules. Keep this document focused on memory maintenance.

Reusable project skills may use `skills/<name>/SKILL.md` when a task benefits from explicit activation guidance and a reusable procedure. They remain ordinary Clumsies Memory; the path does not install a host skill.

State which project, environment, or version a rule applies to. For project-specific knowledge, use a path such as `projects/<project>/<topic>.md` when needed to distinguish it from shared knowledge. Directory names do not grant permissions. Do not turn a project exception into an organization-wide rule.

## How to write

Use a descriptive title and open with the conclusion and its applicability. Include the condition and exception alongside the guidance they qualify, so a retrieved section remains understandable on its own.

Add only the details that help someone act correctly: rationale for a decision, prerequisites and a check for a procedure, or symptoms, cause, fix, and verification for a lesson. Include a source and date when they establish authority or freshness. Do not invent evidence or label something verified without checking it.

Use ordinary Markdown. No universal frontmatter or fixed section count is required. Preserve specific details needed for correct use; remove repetition rather than compressing everything into generic advice.

## How to update

Find related memory and read the complete target before editing. A search with no relevant result does not prove that the topic is absent; check known paths and references before creating a duplicate.

- Already covered: make no change.
- A correction or extension: update the canonical document, preserving valid scope, rationale, and exceptions.
- A distinct topic: create a focused document and link related guidance where useful.
- Conflicting claims: compare applicability and evidence. Replace an old rule only when its replacement is established; otherwise describe the uncertainty or ask for the missing decision. A newer timestamp alone does not establish truth.

Make the smallest coherent edit. Re-read the affected sections after editing. On a version conflict, reload and reconcile rather than overwriting concurrent work. Consolidate or remove other documents only within the authorized scope.

## How to retire and report

Remove obsolete instructions from current guidance. If historical reasoning is useful, mark it as superseded and identify the replacement alongside it. Moving a document into an `archive/` folder does not exclude it from retrieval.

Report which resource changed and what changed. A saved Clumsies Draft can affect the current Project before review; it is not organization-wide publication. If no change was needed, say so.

## Why these guidelines?

These defaults adapt published research and engineering experience to Clumsies. The sources explain the design choices; they do not validate this template's effectiveness.

- [Anthropic: Effective context engineering for AI agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents) informs the focus on relevant information and enough detail for correct action.
- [LangChain: How we built Agent Builder's memory system](https://www.langchain.com/blog/how-we-built-agent-builders-memory-system) motivates explicit guidance for deciding what to save and consolidating accumulated notes.
- [Agentic Context Engineering](https://arxiv.org/abs/2510.04618v3) informs the preference for incremental changes that preserve specific knowledge.
