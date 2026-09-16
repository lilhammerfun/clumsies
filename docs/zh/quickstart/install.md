---
title: 安装 Clumsies
description: 从源码安装日常使用的 macOS App，再连接组织。
prev:
  text: 快速开始
  link: /zh/quickstart/
next:
  text: 连接组织
  link: /zh/quickstart/connect
---
# 安装 Clumsies

当前从仓库的 `main` 分支安装 App。`just install-macos` 会完成编译，安装到 **`~/Applications/Clumsies.app`** 并打开。这是常规应用安装，账号、Memory 和 Agent 设置会持续保留。

::: warning WIP：原生 App 仍在开发中
[GitHub 公开 Releases](https://github.com/lilhammerfun/clumsies/releases) 提供的是旧版 CLI，没有当前 App 的安装包。源码安装使用 Debug 编译配置，不代表稳定发行版。
:::

如果团队已经提供当前 App，请按团队说明安装，然后继续[连接组织](/zh/quickstart/connect)。

## 准备 Mac 环境

App 的运行目标是 **macOS 14 及以上版本**。从源码构建需要能够运行所选 Xcode 的较新 macOS，具体请查 [Apple 的 Xcode 系统要求](https://developer.apple.com/xcode/system-requirements/)。

| 工具 | 要求 |
| --- | --- |
| Xcode | 适配当前 Mac 的完整 Xcode 26 或更新版本；仅有 Command Line Tools 不够 |
| Rust | stable 工具链，当前终端可以运行 `cargo` 和 `rustc` |
| Just | 可在终端运行 `just`，用于执行仓库任务 |
| XcodeGen | 2.46.0 或更新版本，可在终端运行 `xcodegen` |

首次打开 Xcode，接受许可协议并安装所需组件。然后将它设为当前工具链；安装位置不同时，请调整路径：

```sh
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -runFirstLaunch
xcodebuild -version
```

已安装 [Homebrew](https://brew.sh/) 时，可以这样安装其余工具：

```sh
brew install just xcodegen rust
```

已有可用的 Rust stable 工具链时，从命令中去掉 `rust`。已经满足要求的工具可以继续使用。

## 编译并安装

首次获取源码时：

```sh
git clone --branch main https://github.com/lilhammerfun/clumsies.git
cd clumsies
just install-macos
```

首次构建会下载 Swift 和 Rust 依赖，可能耗时数分钟。命令会编译 App 及其内置 daemon、验证签名，安装后打开 App。更新已有安装时，会先退出正在运行的 App，再替换应用包；账号、Memory 和设置会保留。

App 内置默认 Server 地址 **`https://app.clumsies.ai`**，登录仍需要该组织已经准入的账号。如果团队使用其他部署，请向管理员取得对应的 Server 地址和访问权限。

这种安装方式连接已有 Server，不需要本地 Docker 环境。参与开发、需要隔离 App 和本地服务时，请阅读[开发流程](/zh/guides/development-workflow)。

## 更新已有安装

在 `main` 分支的源码目录中，先妥善保留本地修改，再执行：

```sh
git pull --ff-only
just install-macos
```

如果 Git 提示存在本地修改或分支已分叉，先处理这些问题，不要为了更新而丢弃修改。

App 完成 Codex 插件更新后，重启 Codex 并新建任务。如果提示审查 Hook，在 `/hooks` 中查看 Clumsies。快速开始的后续页面会介绍具体接入方法。

## 安装中断时

| 现象 | 处理方式 |
| --- | --- |
| `xcodebuild` 找不到完整 Xcode，或提示缺少首次运行组件 | 核对当前 Xcode 路径，并完成它的初始化。 |
| 找不到 `just`、`xcodegen`、`cargo` 或 `rustc` | 补齐工具或修正当前终端的 `PATH`，再执行安装命令。 |
| 下载依赖或构建失败 | 保留失败的命令和错误输出，按[排查问题](/zh/guides/troubleshooting)继续，或报告故障。 |
| App 能打开，但登录被拒绝 | 安装已经完成，请组织管理员核对账号访问权限。 |

**`~/Applications/Clumsies.app`** 能打开后，继续[连接组织](/zh/quickstart/connect)。
