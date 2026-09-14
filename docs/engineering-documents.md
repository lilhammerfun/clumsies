# Writing documentation

The acceptance test is a reader encountering Clumsies for the first time: **they can follow the reading path to understand the product, design, data, and interfaces, then find implementation evidence when needed.** Accuracy and completeness do not require presenting every internal detail on the first page.

## Organize around readers' questions

| Content type | Question | Entry point |
| --- | --- | --- |
| Introduction | What is it, and why is it designed this way? | [Overview](/overview), [Architecture](/architecture) |
| Structures and principles | What are the entities, fields, relationships, and invariants? | [Data structures](/data-model), [Flows](/flows) |
| Task guide | How do I complete a task and confirm success? | [Guides](/guides/) |
| Interface reference | What are the inputs, outputs, permissions, preconditions, errors, and version rules? | [Domain interfaces](/reference/domain-api), [HTTP](/reference/http-api), [MCP](/mcp) |
| Design detail | How does a subsystem implement these behaviors? | Server, Runtime, Memory, Review, and retrieval pages |
| History and evidence | What changed previously? What did a measurement establish? | Historical pages, [Performance](/performance/) |

Keep one current detailed explanation per topic. Introductory pages explain the necessary concepts and link to it rather than duplicating the specification. English and Chinese use matching routes with equivalent main-path depth and facts.

## Write each page for its purpose

1. Start with what the page helps a reader understand or do, and any necessary prerequisites.
2. Explain concepts through a concrete scenario before introducing entity and field names. Use the fictional deployment rollback checklist example, never a user's private data.
3. Describe identity, ownership, key fields, nullable values, relationships, and version boundaries. Distinguish database rows, HTTP responses, and local derived objects.
4. Group interfaces by domain and use case. Label complete requests versus excerpts and explain where placeholder IDs, hashes, and ETags come from.
5. Show persistence points, completion conditions, failure results, and recovery across processes. Separate local acceptance, synchronization, and publication.
6. Use diagrams to explain relationships. Supply equivalent prose, image alt text, and a full-size link. Check entities, arrows, and states against implementation.
7. End with a few precise source or test links. Preserve historical URLs where useful, but clearly identify retired behavior at the top of those pages.

## Verify the facts

Implementation evidence includes code, database migrations, HTTP routes, Rust/Swift types, and executable tests. OpenAPI route coverage does not establish that every schema field matches runtime behavior; known differences belong in [HTTP contracts](/reference/http-api).

Do not describe design goals as implemented guarantees or local measurements as production performance promises. Current pages explain implemented behavior; dated performance evidence states its scope and limitations. This documentation reorganization was checked against code baseline `5d038ff`; evidence links can pin that revision. Update explanations and links when the corresponding implementation changes.

## Check before submission

- Walk the main path from the [documentation home](/). Are terms explained before use? Does the next page answer the questions raised by this one?
- Compare examples, state transitions, permissions, and errors with code; verify the two languages describe the same behavior.
- Build the site and check navigation, content links, images, and JSON examples:

```sh
bun install --frozen-lockfile
bun run build
bun dev/check-docs-search.mjs
```

- For HTTP contracts, run the existing route-coverage test:

```sh
cargo test -p server --lib axum_routes_match_public_and_admin_openapi
```

- Check desktop and narrow-screen reading. Tables should scroll horizontally and diagrams should open at full size. A successful build does not replace editorial review or API behavior tests.

## Publish to the documentation site

VitePress sources live in `docs/`, static images in `docs/public/`, and build output in `docs/.vitepress/dist/`. The existing Site Delivery workflow builds and deploys documentation changes that land on `main` to docs.clumsies.ai. See the [Site Delivery workflow](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/.github/workflows/site-delivery.yml). A local preview does not mean the live site has changed.
