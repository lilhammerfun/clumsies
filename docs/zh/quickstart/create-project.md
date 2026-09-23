---
description: 创建 Payments 项目，把本机练习仓库绑定到项目，为选择和使用组织 Memory 做准备。
prev:
  text: 连接组织
  link: /zh/quickstart/connect
next:
  text: 选择已有 Memory
  link: /zh/quickstart/select-memory
---

# 1. 创建项目并绑定仓库

这一页把本机的 `clumsies-demo` 仓库与 **Payments** 项目连接起来。之后，Codex 在这个仓库中工作时，Clumsies 就能知道应该使用哪个项目的 Memory。

## 开始前

先完成[连接组织](/zh/quickstart/connect)，并在 App 中登录。教程使用 Payments 作为项目名；你可以在自己有权使用的组织中跟随操作。

普通组织成员可以创建项目，创建者自动成为项目的 **Owner（所有者）**。项目设置会显示每位成员的角色：**Owner（所有者）**、**Maintainer（维护者）**或 **Member（成员）**。Owner 和 Maintainer 可以管理成员、选用组织 Memory；成员操作不能移除 Owner 或降低其角色。发布组织级内容仍需要组织 owner/admin。

如果团队已经为你准备了项目，可以直接选择它，跳到下方“绑定已有项目”。

## 准备练习仓库

如果已有练习仓库，直接使用它。否则打开 macOS **Terminal**，运行：

```sh
mkdir "$HOME/clumsies-demo" && git -C "$HOME/clumsies-demo" init
```

这会在个人目录中创建新的 `clumsies-demo`，并将它初始化为 Git 仓库。若目录已存在，命令会停止；你可以使用已有的练习仓库，不必重复创建。若 macOS 提示安装命令行工具，安装后再运行 `git -C "$HOME/clumsies-demo" init`。

## 创建 Payments

1. 在 macOS 菜单栏选择 **File → New Project…**，或按 **⇧⌘N**。
2. 在 **Name** 输入 `Payments`。**Description** 可以填写项目用途，也可以留空。
3. 展开 **Additional options**，将 **Initial memory** 保持为 **None**。下一步会从 Org 中选择已有 Memory。
4. 点击 **Attach Repositories…**，选中本机的 `clumsies-demo` 目录，再点击系统选择框中的 **Attach**。
5. 点击 **Create**。创建成功后，App 会进入 Payments。

项目是团队共享的对象，仓库绑定记录的是这台 Mac 上的目录位置。其他成员使用同一个项目时，需要在自己的 Mac 上绑定自己的仓库目录。

### 绑定已有项目

如果你使用的是已有项目，或者创建时没有附加仓库：

1. 打开左侧 **Memory**，在顶部项目筛选器中选择 **Payments**。
2. 点击旁边的齿轮按钮 **Project Settings**。
3. 在 **Repositories** 中点击 **Add Repositories…**。
4. 选中 `clumsies-demo` 目录，点击 **Add**。

## 确认完成

在 **Memory → Payments → Project Settings** 中，项目名称应为 Payments，**Repositories** 应列出 `clumsies-demo` 及其本机路径。

此时 Memory 列表为空也正常。创建项目和绑定仓库并不会自动选入组织知识；下一步才决定这个项目需要哪些内容。

## 常见阻碍

- **已有同名项目**：选择团队已有的 Payments；若无访问权限，请项目管理员添加你为成员。
- **New Project… 不可用**：先确认 App 已完成登录和加载；当前账号需要具备创建项目权限。
- **看不到仓库绑定区域**：从 Memory 的项目筛选器选中 Payments，再打开旁边的 Project Settings。这里显示的是当前项目的本机绑定。

下一步：[选择已有 Memory](/zh/quickstart/select-memory)。
