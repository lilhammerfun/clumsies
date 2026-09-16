# Connect a coding agent

Use this guide to enable or repair an agent integration on your Mac. To try the complete workflow with Codex, follow [Use project Memory in Codex](/quickstart/use-with-agent).

An integration makes the Clumsies tools available to the agent. A repository binding determines which Project those tools can access. You need both.

## Before you start

- Open Clumsies and sign in to your organization.
- Install the agent host you want to use. The Codex integration requires the macOS Codex App.
- [Bind your repository to a Project](/quickstart/create-project), then [select its organization Memory](/quickstart/select-memory).

## Enable the integration

Open **Settings → Agents**. Under **Agents on This Mac**, enable the hosts you use. Clumsies installs each integration for your macOS user; you do not install it again for every Project.

On first launch, the **Connect Your Agents** screen offers the same choice, with Codex selected by default. Choose **Install and Continue**, or **Set Up Later** to return through Settings.

For Codex, look for **Plugin installed and enabled**. If it says **Will install when Codex is available**, install the Codex App first. If it says **Plugin needs repair** or **Plugin not installed**, choose **Repair Selected Integrations** and check the status again.

## Start a fresh task

After installing or updating the Codex plugin, restart Codex and start a new task from the bound repository. Existing tasks keep their previous plugin snapshot.

Activity reads Codex and DSH session logs and links Memory activations to local retrieval history. It does not require lifecycle hooks.

Clumsies checks the task's directory at startup and on every tool call. An unbound directory cannot use the Project merely selected in the App. Git worktrees can resolve through the main checkout's binding. The MCP connection keeps its startup directory; a shell `cd` does not retarget it. See [Workspace binding](/guides/workspace-binding) for routing and upgrade details.

## Verify the connection

Ask the agent a concrete question about a document selected for the Project. Check the actual Clumsies **memory** tool result for the document's path and relevant content.

The integration instructs Codex to retrieve relevant context at the start of substantive work. If no retrieval occurs, explicitly ask it to use Clumsies, then inspect the tool call. A statement that it “used memory” is not sufficient evidence. The [Codex tutorial](/quickstart/use-with-agent) provides an example and expected results.

The first use downloads local search models and prepares the Project index. While preparation is in progress, retrieval can return `search_model_preparing`; allow preparation to finish, then retry. Models are cached locally for later use.

## If the connection fails

| What you see | What to check |
| --- | --- |
| No Clumsies tool in a Codex task | Check the plugin status, restart Codex, and create a new task. |
| Repository is not bound | Bind the actual directory used by this task to the intended Project. |
| Runtime version mismatch after an update | Restart Clumsies and the agent host so the bundled runtime and resident daemon use the same build. |
| Retrieval is preparing or a document is missing | Check model preparation, synchronization, and the Project's selected Memory. |
| Retrieval works but activity records are absent | Check that the bound workspace has a supported Codex or DSH session log containing a Clumsies activation. |

If a failure persists, use [Troubleshooting](/guides/troubleshooting) to collect the relevant diagnostics.

## Other supported hosts

Settings also lists Claude Code, opencode, Antigravity, and dsh. For dsh, **Enabled; MCP profile setup required** means you still need to register MCP in its user-managed profile; see [DSH integration](/guides/dsh-integration).

The [adapter reference](/adapter) describes each host's MCP configuration and profile requirements. Use that page when maintaining an integration; the [MCP reference](/mcp) defines the Memory tool itself.
