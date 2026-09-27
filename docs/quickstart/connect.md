---
title: Connect to your organization
description: Sign in to the configured organization and choose the agents to connect on this Mac.
prev:
  text: Install Clumsies
  link: /quickstart/install
next:
  text: Create a project
  link: /quickstart/create-project
---
# Connect to your organization

Open the [installed Clumsies App](/quickstart/install). This page gets you into the organization's workspace and through the initial agent selection. You can then create a Project and choose the Memory it uses.

## Check your account access

Access is invitation-only. An administrator can create a one-time invitation in **Settings → Organization → Members** and share it privately. Choose **Accept invitation** in the App, paste the credential, and set your username and password. No email address or mail service is required. Invitations expire after seven days and can be revoked or reissued.

Administrators can also admit a verified email for identity-provider login. That option records membership without sending mail. An existing local account must explicitly bind its Google account; matching email text never merges accounts.

The default Server is **`https://app.clumsies.ai`**. Installing the App does not grant access to that organization. If your team uses another deployment, obtain its Server address and account access from an administrator.

## Sign in

On **Sign in to Clumsies**, check **Server address**. A first installation prefills the default address; keep it when joining that organization. For another deployment, enter your team's HTTPS address. The documentation website is not a Server address.

Enter your username and password, or select **Sign in with Google** when Google is configured. Other OIDC providers use **Sign in with identity provider**. External authentication opens the system browser and returns to the App. The App loads the organization configured on that Server.

In **Settings → Login methods…**, add a password or explicitly bind your Google account after verifying your current account. Both methods keep the same user, projects, and permissions.

For a forgotten password, ask an administrator for a reset credential and choose **Forgot password?**. Reset credentials expire after 30 minutes; resetting your password revokes old sessions. There is no email recovery service.

## Choose the agents on this Mac

On first use, **Connect Your Agents** asks which agent integrations to enable. Codex is selected by default. For this quickstart, install the macOS Codex App separately, keep Codex selected, and choose **Install and Continue**. If it is not installed yet, you can choose **Set Up Later** and return to **Settings → Agents**.

Adapters are installed once for this Mac user and work across projects. The repository you bind later determines which Project's Memory Codex uses. Enabling an adapter alone does not bind a repository.

After the Codex integration is installed or changed, restart Codex and start a new task. The [Use Memory in Codex](/quickstart/use-with-agent) page explains how to verify retrieval after you select the project's Memory.

## Check the workspace

Confirm that Clumsies shows the intended organization. Organization Memory contains the team's published knowledge; a Project chooses which of that knowledge it uses. Local retrieval models and indexes may still be preparing after sign-in. You can continue setting up the Project while they finish.

## If you cannot continue

| What you see | What to do |
| --- | --- |
| **Set up Clumsies Server** | This Server has not been initialized. Confirm the address if you meant to join an existing organization. A first administrator follows [Organization deployment](/guides/deploy-for-an-org). |
| Sign-in is denied | Ask an administrator to check your member email, account status, and the organization's email-domain policy. Check that the identity provider verifies your email. |
| The browser does not complete the callback, or the App reports a connection error | Keep the error message, check the address and network, then use [Troubleshooting](/guides/troubleshooting). |
| Codex integration is not ready | Confirm that the Codex App is installed, then check **Settings → Agents** and [Agent integration](/guides/agent-runtime). |

When the intended organization is open, continue to [Create a project](/quickstart/create-project).
