//! What a Project's Memory starts from.
//!
//! macOS ships these four documents as bundled resources and offers them from
//! its empty Memory state as "Set Up Guidelines": CLUMSIES.md, which tells agents
//! what is worth remembering and how to keep it useful, and one README per starter
//! folder giving that folder a purpose. The text is part of the product rather
//! than of either platform, so it is ported here as it stands.
//!
//! Every document is proposed as a draft: a Project's Memory changes through
//! review, and the starting point is no exception.

/// The guidelines file. The daemon's Project config names a path for it too, and
/// this is the default that config falls back to.
pub const GUIDELINES_PATH: &str = "CLUMSIES.md";

/// One document of the starter set.
pub struct Starter {
    pub path: String,
    pub body: &'static str,
}

/// The documents a Project with no Memory can start from: the guidelines, and a
/// README for each starter folder that is not already in use.
pub fn starters(occupied: &[String]) -> Vec<Starter> {
    let mut starters = Vec::new();
    // A Project that already has guidelines keeps them: the empty state is the
    // only place this is offered from, and it cannot have one.
    if !occupied.iter().any(|path| path == GUIDELINES_PATH) {
        starters.push(Starter {
            path: GUIDELINES_PATH.to_owned(),
            body: GUIDELINES,
        });
    }
    for (folder, body) in [
        ("knowledge", KNOWLEDGE),
        ("procedures", PROCEDURES),
        ("lessons", LESSONS),
    ] {
        let used = occupied
            .iter()
            .any(|path| path == folder || path.starts_with(&format!("{folder}/")));
        if !used {
            starters.push(Starter {
                path: format!("{folder}/README.md"),
                body,
            });
        }
    }
    starters
}

const GUIDELINES: &str = r#"# Memory Guidelines

This document tells agents what is worth remembering and how to organize, update, and retire knowledge in this memory space.

Keep memory useful for future work: specific, scoped, supported, and easy to update. Apply these defaults when the user requests memory maintenance. Follow explicit user instructions and established project conventions. This document does not authorize additional writes.

## What to keep

Keep durable knowledge that would change a future answer or action: decisions and their reasons, project constraints, verified procedures, and lessons that prevent a known failure.

Unless the user requests a particular record, leave out temporary progress, raw conversations, large logs, unsupported guesses, and information already easy to recover from the maintained source. Link to that source and retain the non-obvious context. Never include credentials or secrets.

## Where it belongs

Preserve the existing organization. Default setup creates these folders with a short README explaining their purpose:

- `knowledge/<topic>.md` for facts, constraints, and decisions.
- `procedures/<task>.md` for repeatable steps and their verification.
- `lessons/<failure>.md` for a verified failure, its cause, and prevention.

The folder READMEs are editable orientation notes, not project knowledge or evidence. Add only the documents needed. Use one canonical document per independently maintainable topic. Link related documents instead of copying their rules. Keep this document focused on memory maintenance.

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
"#;

const KNOWLEDGE: &str = r#"# Knowledge

Keep facts, constraints, and decisions that should guide future work in this folder.

Use one descriptive `<topic>.md` per independently maintainable topic. State its scope, the conclusion, and the reasons or evidence behind it. Update the existing topic when knowledge changes.

Follow [Memory Guidelines](../CLUMSIES.md) when the user requests memory maintenance. This page explains the folder; it is not a record of project facts.
"#;

const PROCEDURES: &str = r#"# Procedures

Keep repeatable, verified ways to complete a task in this folder.

Use a descriptive `<task>.md`. Include when it applies, prerequisites, the steps that matter, and how to check the result. Record environment or version limits alongside the steps they affect.

Follow [Memory Guidelines](../CLUMSIES.md) when the user requests memory maintenance. Add a procedure only after checking it; this page does not establish any verified procedure.
"#;

const LESSONS: &str = r#"# Lessons

Keep verified failures and the knowledge needed to prevent them in this folder.

Use a descriptive `<failure>.md`. Explain the symptoms, cause, fix, verification, and conditions where the lesson applies. Link to evidence and related procedures instead of copying them.

Follow [Memory Guidelines](../CLUMSIES.md) when the user requests memory maintenance. This page explains the folder; it is not evidence that a failure occurred.
"#;

#[cfg(test)]
mod tests {
    // Only what the tests use: the component library exports a `test` macro of
    // its own, and a glob import would shadow the built-in attribute with it.
    use super::starters;

    #[test]
    fn a_project_with_nothing_starts_from_the_guidelines_and_three_folders() {
        let paths: Vec<String> = starters(&[])
            .into_iter()
            .map(|starter| starter.path)
            .collect();
        assert_eq!(
            paths,
            [
                "CLUMSIES.md",
                "knowledge/README.md",
                "procedures/README.md",
                "lessons/README.md"
            ]
        );
    }

    #[test]
    fn guidelines_a_project_already_has_are_left_alone() {
        let occupied = ["CLUMSIES.md".to_owned()];
        let starters = starters(&occupied);
        assert!(!starters.iter().any(|s| s.path == "CLUMSIES.md"));
    }

    #[test]
    fn a_folder_the_project_already_uses_is_left_alone() {
        let occupied = ["knowledge/architecture.md".to_owned()];
        let paths: Vec<String> = starters(&occupied)
            .into_iter()
            .map(|starter| starter.path)
            .collect();
        assert!(!paths.contains(&"knowledge/README.md".to_owned()));
        assert!(paths.contains(&"procedures/README.md".to_owned()));
    }
}
