# Interface and reference guide

Use this section to connect a product action to its data and interface. If you are new to Clumsies, first read [Overview](/overview), [Architecture](/architecture), [Data model](/data-model), and [Core flows](/flows). You can then return here for exact fields, permissions and failure handling.

## Find the right page

| Your question | Read |
| --- | --- |
| Which domain owns this capability, and which interface should I call? | [Domain interfaces](/reference/domain-api) |
| What do the HTTP requests look like? Which version or ETag do I send? | [HTTP contracts and examples](/reference/http-api) |
| How does an Agent discover, load and propose Memory? | [MCP: the Memory tool](/mcp) |
| How do login, roles, token refresh and local credentials work? | [Authentication and sessions](/reference/auth) |
| What does a project-specific term mean? | [Glossary](/glossary) |
| Which local files does Clumsies write? | [Runtime surfaces](/runtime) |
| How do Coding Agent hosts start the local integration? | [Agent runtime](/guides/agent-runtime) |
| Where does the implementation live? | [Codebase map](/repos) |

## Machine-readable HTTP contracts

| Contract | Intended caller |
| --- | --- |
| [Public OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.public.v1.yaml) | Authenticated product clients, normally daemon |
| [Admin OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.admin.v1.yaml) | Organization Administration; also describes health and first-installation setup |

The OpenAPI route inventory is checked against Server. Some declared pagination and conditional-read behavior is ahead of the current handlers; see [contract limits](/reference/http-api#contract-limits) before building an integration. Local XPC and MCP use their own executable contracts rather than these HTTP schemas.
