# Writing documentation

A new reader should be able to choose a useful reading route, understand the system, and find reliable instructions or contracts without reading the source first. Completing a tutorial and understanding the architecture are different needs; the documentation must serve both.

## Organize around readers' questions

Give each page one primary purpose. A short explanation can support an action, and an example can clarify a contract, but an extended topic belongs on its own page.

| Page type | What belongs here | Clumsies entry |
| --- | --- | --- |
| Tutorial | A defined starting point, one guided learning exercise, small steps, and visible results | [Quickstart](/quickstart/) |
| Task guide | A specific goal, required access, relevant choices, actions, verification, and recovery | [Task guides](/guides/) |
| Concept or design explanation | The problem, relationships, ownership, reasons for decisions, tradeoffs, and limits | [Overview](/overview), [Architecture](/architecture), [Data model](/data-model), [Flows](/flows) |
| Reference | Precise operations, fields, types, permissions, preconditions, errors, and version rules | [Domain interfaces](/reference/domain-api), [MCP](/mcp), [HTTP](/reference/http-api) |
| Index | Who a route serves, its reading order, prerequisites, and expected result | [Home](/), [Guide index](/guides/), [Reference index](/reference/) |

Development and maintenance are reader groups, not exceptions to these page responsibilities. A development setup page is a task guide; a runtime ownership page is an explanation. Mark historical material with its date or applicable version and link to the current behavior.

## Maintain the reading routes

The home page serves three groups: team members using Memory, technical readers learning the design, and people integrating, operating, or developing Clumsies. Each route names what a reader should be able to do or explain afterward.

Keep the five tutorial steps in this order: create a Project → select existing organization Memory → use it with Codex → explicitly ask Codex to propose an update → human Review and publication, followed by verification in Codex. Installation and sign-in are preparation, without step numbers.

Use Payments, `clumsies-demo`, and `deployment-rollback.md` consistently. The document must already exist in Organization Memory; it is not built-in sample data. Link an empty organization to [Create Memory](/guides/create-memory), then return to the tutorial. Selection references a shared document; it does not copy its content.

Check these relationships whenever navigation changes:

- Previous and Next links agree with the intended reading order in both languages.
- A page that requires a previous result links to the page that produces it.
- Subsystem details and history remain behind clear, collapsed groups.
- Published URLs and useful heading anchors remain reachable. Move their readers to the current explanation instead of retaining another complete tutorial.
- English and Chinese describe equivalent prerequisites, examples, roles, and outcomes.

## Write each page for its purpose

**Tutorials:** state what the reader will complete and how to recognize progress. Introduce terms where they become necessary. Move installation alternatives, full API payloads, and design discussions to linked pages.

**Task guides:** start with the actual problem, such as selecting shared knowledge or recovering a failed synchronization. State the required starting condition and any decisions that change the steps. End with a check of the result and a useful recovery path. Do not repeat the entire first-use sequence.

**Explanations:** connect concepts before listing implementation names. Explain what is authoritative, what is derived, why a boundary exists, and what that choice costs. Use a concrete example and an accurate diagram where it helps; include equivalent prose. A table of entity names alone does not explain a data model.

**References:** use a consistent structure so a reader can find an input, permission, error, or concurrency rule directly. Distinguish a database row from an HTTP response or local object. Mark excerpts and placeholders, and explain how callers obtain real IDs, hashes, or versions.

**Indexes:** link to the maintained detail. If an index grows into a second explanation or tutorial, shorten it. The published [member workflow URL](/guides/how-to-use-clumsies) is now a task index.

## Verify the facts

| Claim | Evidence to check |
| --- | --- |
| An action exists and a role may perform it | Current action code, authorization handlers, and relevant tests |
| A field or response has a particular meaning | Types or migrations together with the handler or serializer that uses them |
| Saving, synchronization, or publication has completed | Transaction boundaries, queue handling, state transitions, and failure tests |
| A caller can recover or retry safely | Error paths, idempotency or version checks, and documented limits |
| An App installer is available | Actual release assets and the supported installation route |

A design proposal or an old document is context, not proof of current behavior. Route coverage alone does not establish schema accuracy. Record what revision and evidence were checked in the change description or the relevant page; do not imply that an old baseline verifies every page after the implementation changes.

Use current implementation names only where they help the reader. Keep local saved state, Server synchronization, and shared publication distinct. Check Project roles separately from organization roles. Explain whether a retrieval result is a fragment or a full document.

Label planned behavior as planned. Scope performance evidence to its environment and date. Do not describe a static source review as a completed App test, or refer to an illustration or other asset that is absent.

## Review as a first-time reader

A reviewer should be able to answer these questions from the relevant pages:

| Reader | Acceptance questions |
| --- | --- |
| Team member | Where do I start? What must already exist? Does selection copy a document? When may Codex propose a change? Who publishes it, and how do I verify the result? |
| Technical reader | What belongs to the App, daemon, and Server? Which objects represent content and proposals? What changes when a version is published? What can fail between those steps? |
| Integrator or maintainer | Which interface serves my operation? What access and version conditions apply? Where are errors, recovery rules, implementation, and tests documented? |

Also check whether each unfamiliar term is explained or linked before it becomes necessary, whether the example stays consistent across pages, and whether the next link answers the reader's likely next question.

## Check before submission

Review the facts and both reading routes first. Then run the relevant existing checks:

```sh
bun install --frozen-lockfile
bun run build
bun dev/check-docs-search.mjs
```

For HTTP route changes, also run:

```sh
cargo test -p server --lib axum_routes_match_public_and_admin_openapi
```

Check internal links, heading anchors, language counterparts, example requests, and referenced assets. Choose behavior tests according to the claim being changed. A successful site build confirms that pages can be generated; it does not prove that instructions or API behavior are correct.

## Publish to the documentation site

VitePress sources live in `docs/`, static assets in `docs/public/`, and build output in `docs/.vitepress/dist/`. Follow the repository's [Site Delivery workflow](https://github.com/lilhammerfun/clumsies/blob/main/.github/workflows/site-delivery.yml) for publication. Distinguish local preview, completed delivery, and verification of the live site.

## Sources for this structure

- [Diátaxis](https://diataxis.fr/) distinguishes learning, completing work, understanding, and looking up information. Its pages on [tutorials](https://diataxis.fr/tutorials/), [task guides](https://diataxis.fr/how-to-guides/), [explanation](https://diataxis.fr/explanation/), and [reference](https://diataxis.fr/reference/) inform the page responsibilities above.
- [Kubernetes documentation](https://kubernetes.io/docs/home/) provides separate Concepts, Tasks, Tutorials, and Reference entrances. Clumsies likewise keeps explanations and contracts independently accessible.
- [Docker Get started](https://docs.docker.com/get-started/) separates installation from choosing a tutorial by goal. Clumsies places installation and sign-in before the five-step exercise.
- [GitHub Get started](https://docs.github.com/en/get-started) offers quickstart, overview, and specific topics together. Clumsies uses distinct routes for first use, understanding the project, and finding a task.

These are methods for organizing the material. Clumsies examples, roles, operations, and guarantees must come from this project's current behavior.
