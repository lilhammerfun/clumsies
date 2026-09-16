---
description: Create Payments and bind a local practice repository before selecting and using organization Memory.
prev:
  text: Connect to your organization
  link: /quickstart/connect
next:
  text: Select existing Memory
  link: /quickstart/select-memory
---

# 1. Create a Project and bind a repository

This page connects your local `clumsies-demo` repository to **Payments**. When Codex works in that repository, Clumsies can identify which Project's Memory to use.

## Before you start

Complete [Connect to your organization](/quickstart/connect) and sign in to the App. Payments is the example Project name; follow along in an organization you are authorized to use.

In the current version, ordinary organization members can create Projects. The creator becomes a **Project admin**, able to manage Project membership and select organization Memory. Publishing organization content still requires an organization owner/admin.

If your team has prepared a Project for you, select it and skip to “Bind an existing Project” below.

## Prepare a practice repository

Use an existing practice repository if you have one. Otherwise, open macOS **Terminal** and run:

```sh
mkdir "$HOME/clumsies-demo" && git -C "$HOME/clumsies-demo" init
```

This creates a new `clumsies-demo` directory in your home directory and initializes it as a Git repository. If the directory already exists, the command stops; you can use your existing practice repository. If macOS prompts you to install command line tools, install them, then run `git -C "$HOME/clumsies-demo" init`.

## Create Payments

1. Choose **File → New Project…** in the macOS menu bar, or press **⇧⌘N**.
2. Enter `Payments` in **Name**. Describe its purpose in **Description**, or leave it blank.
3. Expand **Additional options** and leave **Initial memory** set to **None**. You will select existing Org Memory next.
4. Click **Attach Repositories…**, select your local `clumsies-demo` directory, and click **Attach** in the folder picker.
5. Click **Create**. The App switches to Payments when creation succeeds.

The Project is shared by the team. The repository binding records a directory on this Mac. Teammates using the same Project bind their own repository directories on their own Macs.

### Bind an existing Project

If you are using an existing Project, or did not attach a repository during creation:

1. Open **Memory** in the sidebar and select **Payments** in the top Project filter.
2. Click the adjacent **Project Settings** gear button.
3. Under **Repositories**, click **Add Repositories…**.
4. Select `clumsies-demo` and click **Add**.

## Check your result

Open **Memory → Payments → Project Settings**. The Project name should be Payments, and **Repositories** should list `clumsies-demo` and its local path.

An empty Memory list is expected. Creating a Project and binding a repository do not select organization knowledge automatically; the next step chooses the content this Project needs.

## Common obstacles

- **The Project name is already used**: select the team's existing Payments Project. Ask its administrator to add you if you do not have access.
- **New Project… is unavailable**: confirm the App has finished signing in and loading. Your account needs permission to create Projects.
- **The repository section is missing**: select Payments in Memory's Project filter, then open the adjacent Project Settings. This view shows the current Project's local bindings.

Next: [Select existing Memory](/quickstart/select-memory).
