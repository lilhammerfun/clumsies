# Use Clumsies with your repository

This guide takes a team member from sign-in to a reviewed Memory change. It uses the **Deployment rollback checklist** example from the [overview](/overview).

The macOS App includes the default Server address, `https://app.clumsies.ai`. Sign-in automatically loads the organization configured on that Server. An organization owner or administrator manages account admission and Project access. If you are setting up a new Server, start with [deployment](/guides/deploy-for-an-org).

## 1. Sign in to your organization

Open Desktop, keep the prefilled **Server address**, and click **Continue in Browser** to complete SSO sign-in. An existing installation remembers the Server address you previously used.

Change the address only when connecting to another deployment, using the Server origin supplied by its administrator. A Server origin looks like `https://memory.example.com`: no extra page path, query, or embedded credentials. Remote connections require HTTPS; loopback HTTP is supported for local development.

For a new installation, Desktop instead offers setup using the deployment Setup Code. The first verified identity becomes the organization owner. Normal members join the existing installation; they do not initialize another one.

After sign-in, confirm that Desktop shows the intended organization and account.

## 2. Choose a Project and attach the repository

Select the Project for your work. If no Project is available, an administrator must grant access or create one.

In the Project's **Repositories** section, use **Add Repositories…** to attach your local repository. This creates a binding on this Mac. Other Macs need their own bindings because their repository paths may differ.

A Project and a repository are different things:

- The Project is the shared working context, membership, and Memory selection.
- The binding tells the local daemon which Project applies to a repository directory.
- Selecting a Project in Desktop controls what you are browsing. It does not replace the repository binding used by managed agent integrations.

## 3. Check the Memory selection

Open **Memory** and compare the two views:

| View | What to expect |
| --- | --- |
| **Organization** | Published shared Memory |
| **Your Project** | Selected organization Memory, with local pending Draft changes applied |

For the example, the Project should include `operations/deployment-rollback.md`. An organization owner/admin changes the Project's organization-Memory selection. A member can propose edits to selected Memory through a Project Draft.

If a shared document is missing from your Project, check the selection before troubleshooting search. Finding it in the organization library does not mean it is selected for every Project.

A **Bundle** is a personal saved selection of shared Memory IDs. It helps reuse a set of documents; it does not publish another copy or override the Project's selection.

## 4. Connect your agent host

Use Desktop's agent integration settings to install the supported host integration. The [Agent runtime guide](/guides/agent-runtime) covers the host-specific details.

The integration launches the App-bundled MCP proxy. The resident daemon supplies the Project's Memory. The agent does not need a separate Clumsies database, Server token, or manually synchronized Markdown folder.

Start a task from the bound repository and ask the agent to find the team's deployment rollback guidance. The usual tool sequence is:

```text
activate: find relevant Memory fragments for the task
load: read the complete checklist when its details matter
store: propose a change only when the user asks to maintain Memory
```

The actual MCP surface is one tool named `memory`, with three operations. See [MCP](/mcp) for call syntax.

Managed host-plugin runtimes stop if the binding is missing or changes during a task. Correct the binding, then start a new task. A manually launched plain `mcp serve` retains a compatibility fallback to the Project selected in Desktop; use the managed binding workflow for predictable project selection.

On first use, local retrieval models and the index may still be preparing. Inspect the reported progress and retry when ready.

## 5. Make one proposed improvement

Open the checklist in the Project's Memory view and add the missing verification step. Alternatively, explicitly ask the agent to update that Memory. For an agent edit, it must first load the current document and use the returned hash and exact text.

The change creates or reuses a **Draft**. Saving it sends an operation to the daemon's local durable queue. The daemon automatically synchronizes it to Server.

Check the state instead of treating every success as publication:

| State | Meaning |
| --- | --- |
| Locally saved / queued | The proposal is durable on this Mac; upload may still be pending |
| Synchronized Draft | Server has the proposal; organization Memory is unchanged |
| Merged Review | The proposal has become a new organization version |

Changes are available through the Project's Effective Memory as the local read/index pipeline catches up. They are not shared published guidance yet.

## 6. Resolve shared updates and request Review

If Desktop shows **A shared update is available**, use **Merge latest version** to compare Base, Current, and Draft. Read the resulting content and confirm. Resolve overlapping edits in the same screen.

Then use **Request Review** for one document, or **Request Review for All Project Changes…** to submit the relevant Project Drafts together. Check the file list, proposed results, and explanation before submitting.

The submission flow synchronizes pending operations and coordinates behind Drafts using valid comparison candidates. If another update arrives during confirmation, refresh and inspect the new comparison.

Once submitted, open the Review and discuss the changes. Submission success confirms the Review exists; the detail page still needs to fetch the data required to show its diffs.

## 7. Have an authorized reviewer publish it

An organization owner or administrator reviews the complete proposal:

- **Approve and merge** publishes the ordered Draft set as one organization Commit.
- **Reject** returns the Drafts for further editing and resubmission.

The current Desktop approval action combines approval with merge. The API also supports a separate Approved state; a Review in that state is not published until it is merged.

If the organization Ref has advanced, the reviewer must use an updated, coordinated proposal. Refreshing a stale screen is part of making a decision against the right version.

After merge, the originating Project and other affected Projects receive new snapshots. Each Mac then synchronizes its snapshot and retrieval index. Ask the agent to retrieve again when the local state is ready; existing conversation text does not update itself.

## When something does not work

| Symptom | First action |
| --- | --- |
| No Project access | Have an organization administrator check membership |
| Agent reports a missing or changed binding | Check the attached repository and restart the agent task |
| Search is preparing | Check model/index progress rather than repeatedly submitting the same call |
| Draft remains queued | Inspect synchronization status and sign-in, then retry the existing operation flow |
| Content changed before an agent edit | Load again and rebuild the exact replacements |
| Review says shared changes need attention | Inspect and confirm the latest Base/Current/Draft comparison |
| Review merged but local content is old | Check Project synchronization and index readiness, then read again |

A failed response can leave the outcome uncertain if the response was lost after Server processed the request. Refresh the existing Draft or Review before creating another proposal. The [end-to-end walkthrough](/flows) explains these boundaries in more detail.

## Local storage and administration

In Settings, the Project local-storage controls show the cache location, size, and availability. Use **Choose…** to relocate it or **Reset** to return to the standard location. Clumsies manages a hidden subtree under the chosen directory; it is not a folder for manually editing Memory.

**Clear Cache…** removes rebuildable Commit generations and the Project search index. It preserves Drafts, pending operations, settings, and unrelated files. When an external location is unavailable, reconnect it or restore access. Clumsies does not silently create another cache elsewhere; checkout and MCP retrieval wait for the configured location.

Owners/admins use **Administration** for members, Projects, tokens, audit, and health. If daemon startup fails, **Administrator Recovery** offers a temporary direct Server session for administrative repair. Normal Memory work still requires the daemon.

## Implementation references

The [Project interface](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/apps/macos/Sources/Features/ProjectManagementView.swift), [workspace actions](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/apps/macos/Sources/Domain/WorkspaceStore.swift), and [Server authorization handlers](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/server/src/changes/http.rs) define this workflow. For the underlying design, continue to [architecture](/architecture) and [data model](/data-model).
