---
title: 安装 Clumsies
description: 下载 macOS DMG 安装 Clumsies、完成首次打开并连接组织，也可选择源码安装。
prev:
  text: 快速开始
  link: /zh/quickstart/
next:
  text: 连接组织
  link: /zh/quickstart/connect
---
# 安装 Clumsies

## 下载并安装

支持 **macOS 14 及以上版本、Apple Silicon 和 Intel Mac**，无需安装编译工具。

1. 打开 [GitHub Releases](https://github.com/lilhammerfun/clumsies/releases)，选择最新 **Clumsies macOS Preview** 中的 `Clumsies-*-macos-universal.dmg`。
2. 打开 DMG，将 `Clumsies.app` 拖入 `Applications`，然后推出磁盘映像。
3. 打开应用。当前体验版尚未经过 Apple 公证；若 macOS 拦截，确认下载来源可信后，到 **系统设置 → 隐私与安全 → 仍要打开**。[Apple 说明](https://support.apple.com/zh-cn/102445)
4. 继续[连接组织](/zh/quickstart/connect)。默认 Server 地址为 `https://app.clumsies.ai`，登录需要组织准入的账号。首次使用会联网下载检索模型。

::: warning 体验版
当前 DMG 使用本地签名，尚未经过 Apple 公证。受管理的 Mac 可能不允许手动放行。它使用正常 App 的账号、Memory 和设置；不是稳定发行版。
:::

更新时先退出 App，下载新的 DMG 并替换原位置的应用，用户数据会保留。此前通过源码安装到 `~/Applications/Clumsies.app` 的用户，请继续在该位置替换，避免安装两份。体验版通过下载 DMG 手动更新。

## 从源码安装

以下工具仅用于自行编译；直接下载 DMG 的用户可以跳过本节。

### 准备 Mac 环境

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

### 编译并安装

首次获取源码时：

```sh
git clone --branch main https://github.com/lilhammerfun/clumsies.git
cd clumsies
just install-macos
```

首次构建会下载 Swift 和 Rust 依赖，可能耗时数分钟。命令会编译 App 及其内置 daemon、验证签名，安装后打开 App。更新已有安装时，会先退出正在运行的 App，再替换应用包；账号、Memory 和设置会保留。

App 内置默认 Server 地址 **`https://app.clumsies.ai`**，登录仍需要该组织已经准入的账号。如果团队使用其他部署，请向管理员取得对应的 Server 地址和访问权限。

这种安装方式连接已有 Server，不需要本地 Docker 环境。参与开发、需要隔离 App 和本地服务时，请阅读[开发流程](/zh/guides/development-workflow)。

### 更新源码安装

在 `main` 分支的源码目录中，先妥善保留本地修改，再执行：

```sh
git pull --ff-only
just install-macos
```

如果 Git 提示存在本地修改或分支已分叉，先处理这些问题，不要为了更新而丢弃修改。

App 完成 Codex 插件更新后，重启 Codex 并新建任务。快速开始的后续页面会介绍具体接入方法。

## 安装中断时

| 现象 | 处理方式 |
| --- | --- |
| 首次打开提示无法验证开发者 | 确认来自本仓库后，在系统设置 → 隐私与安全中选择“仍要打开”；设备管理策略可能限制此操作。 |
| `xcodebuild` 找不到完整 Xcode，或提示缺少首次运行组件 | 核对当前 Xcode 路径，并完成它的初始化。 |
| 找不到 `just`、`xcodegen`、`cargo` 或 `rustc` | 补齐工具或修正当前终端的 `PATH`，再执行安装命令。 |
| 下载依赖或构建失败 | 保留失败的命令和错误输出，按[排查问题](/zh/guides/troubleshooting)继续，或报告故障。 |
| App 能打开，但登录被拒绝 | 安装已经完成，请组织管理员核对账号访问权限。 |

已安装的 **Clumsies.app** 能打开后，继续[连接组织](/zh/quickstart/connect)。
