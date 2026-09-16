---
title: 连接组织
description: 登录服务端配置的组织，并选择这台 Mac 上要接入的 Agent。
prev:
  text: 安装 Clumsies
  link: /zh/quickstart/install
next:
  text: 创建项目
  link: /zh/quickstart/create-project
---
# 连接组织

打开[已安装的 Clumsies App](/zh/quickstart/install)。本篇带你进入组织工作区，完成首次 Agent 选择，然后就可以创建 Project、选择它需要的 Memory。

## 确认账号权限

首次登录前，组织管理员需要先添加你的成员账号。登录时使用对应的 SSO 邮箱；身份提供方必须将邮箱标记为已验证，邮箱也要符合组织设置的域名规则。**Add Member…** 只记录成员信息，不会发送邀请邮件。

默认 Server 地址为 **`https://app.clumsies.ai`**。安装 App 不会自动获得这个组织的访问权限。团队使用其他部署时，请向管理员取得对应的 Server 地址和账号权限。

## 完成登录

在 **Sign in to Clumsies** 界面检查 **Server address**。首次安装会预填默认地址，加入这个组织时保留即可；接入其他部署时，改填团队提供的 HTTPS 地址。文档站地址不是 Server 地址。

点击 **Continue in Browser**，在系统浏览器中完成组织登录，然后返回 Clumsies。App 会加载这个 Server 配置的组织，不需要手动填写组织 ID。

## 选择这台 Mac 上的 Agent

首次使用时，**Connect Your Agents** 会询问要启用哪些 Agent 集成，默认勾选 Codex。本教程使用 macOS Codex App：请先自行安装它，保留 Codex 勾选并点击 **Install and Continue**。尚未安装时，可以选择 **Set Up Later**，以后再到 **Settings → Agents** 完成设置。

适配器按本机用户安装一次，供所有项目使用。之后绑定的仓库决定 Codex 使用哪个 Project 的 Memory；启用适配器不会自动完成仓库绑定。

Codex 集成安装或修改后，需要重启 Codex 并新建任务。选好项目的 Memory 后，[在 Codex 中使用 Memory](/zh/quickstart/use-with-agent)会介绍如何确认检索正常。

## 确认工作区

检查 Clumsies 显示的组织是否正确。组织 Memory 保存团队已发布的知识，Project 决定项目使用其中哪些内容。登录完成后，本地检索模型和索引可能还在准备；这期间可以继续设置 Project。

## 如果无法继续

| 看到的情况 | 处理方式 |
| --- | --- |
| **Set up Clumsies Server** | 这个 Server 尚未初始化。加入已有组织的成员先核对地址；首次部署的管理员按[组织部署](/zh/guides/deploy-for-an-org)完成初始化。 |
| 登录被拒绝 | 请管理员核对成员邮箱、账号状态和组织的邮箱域名规则，并确认身份提供方已验证你的邮箱。 |
| 浏览器没有完成回跳，或 App 显示连接错误 | 保留错误提示，检查地址和网络，再按[排查问题](/zh/guides/troubleshooting)继续。 |
| Codex 集成尚未就绪 | 确认已经安装 Codex App，再检查 **Settings → Agents** 和 [Agent 接入](/zh/guides/agent-runtime)。 |

正确的组织已经打开后，继续[创建项目](/zh/quickstart/create-project)。
