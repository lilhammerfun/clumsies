---
title: Clumsies CLI
description: 安装 Windows/Linux 命令行，接入 Agent，并在没有图形客户端的情况下审阅和发布 Memory。
---
# Clumsies CLI

`clumsies` 是人工操作入口；`clumsiesd` 继续管理凭据、本地 Draft、缓存、目录绑定、同步和现有的 `clumsiesd mcp serve` Agent 入口。两个程序来自同一个 Rust 包，必须一起升级。Windows/Linux 图形客户端开发暂时搁置，macOS 原生 App 继续维护。

## 安装

首批包面向 **Linux x86_64（Ubuntu 24.04 或兼容的 glibc 系统）** 和 **Windows x64**。仓库的 **CLI** 工作流生成并验证压缩包及 Windows 用户安装器，后续正式标签发布会包含这些资产。首个版本发布前，可从该工作流下载构建产物。执行前，用旁边的 SHA-256 文件验证压缩包或安装器。

Linux 解压 `clumsies-cli-VERSION-linux-x86_64.tar.gz`，进入包目录执行：

```sh
sha256sum --check SHA256SUMS
./install.sh
export PATH="$HOME/.local/bin:$PATH"
clumsies daemon start
```

默认程序目录是 `~/.local/lib/clumsies/runtime`，入口链接位于 `~/.local/bin`。可用 `CLUMSIES_INSTALL_ROOT` 和 `CLUMSIES_BIN_DIR` 选择其他用户目录。`install.sh --uninstall` 只删除程序和管理的链接，保留 daemon 数据。无需桌面环境或系统服务；需要系统 C/C++ 运行库和 util-linux 的 `flock`。CI 会检查两个程序的动态库依赖。

Windows 运行 `clumsies-cli-VERSION-windows-x86_64-Setup.exe`。它安装到当前用户的 `%LOCALAPPDATA%\Programs\ClumsiesCLI`，将 `runtime` 目录加入用户 PATH，无需管理员权限。安装后重新打开终端。包内附带所需 MSVC 运行库 DLL；安装器目前没有代码签名，只执行已验证来自本仓库的产物。

便携模式可将 ZIP 解压到可写目录，直接执行其中的 `clumsies.exe`。ZIP 的文件位于压缩包根目录，也可执行：

```powershell
.\install.ps1 -AddToPath
clumsies daemon start
```

`install.ps1 -Uninstall` 删除已安装程序和对应 PATH 项，保留 daemon 数据。用户安装器也可通过 Windows 设置中的应用列表卸载。

macOS App 内嵌 `Contents/Resources/clumsies`，可直接执行或将资源目录加入 PATH。CLI 复用 App 的 launch agent、Keychain、配置和缓存；macOS 不提供独立的 `daemon stop/restart`，由 App 管理运行时。从源码编译的 CLI 也会查找 `/Applications/Clumsies.app` 或 `~/Applications/Clumsies.app`。

## 分页和项目选择

`project list`、`review list PROJECT`、`review comments REVIEW_ID` 和 `draft list` 统一支持 `--limit`（1–200，默认 100）、`--cursor` 和 `--all`。默认只返回一页；将返回的游标原样传入即可继续。`--all` 从第一页开始获取完整列表，不能与 `--cursor` 同用。任一页失败或游标循环时，命令失败，不输出不完整的合并结果。输出继续使用 JSON。

```sh
clumsies project list --limit 20
clumsies project list --limit 20 --cursor '返回的游标'
clumsies project list --all
clumsies review list AgentOS --all
clumsies review comments REVIEW_ID --all
clumsies draft list --status open --limit 20
clumsies draft list --cursor '返回的游标'
```

Server 列表返回 `page_info.next_cursor` 和 `page_info.has_more`，本地 Draft 返回顶层 `next_cursor`；游标为 null 表示结束。项目、Review 和讨论分页需要包含本次改动的 Server；旧 Server 会忽略分页参数，最多返回 200 条。Server 采用 offset 分页，并以 ID 处理相同排序时间；并发修改可能移动记录，修改后应重新列出，不能把跨页结果视为固定快照。`--all` 会在内存中保存合并结果。

项目 `show`、`join`、`bind`、`bindings` 和 `review list` 接受项目 ID 或唯一名称（忽略大小写）。名称查找会读取全部可访问页；未找到或存在歧义时直接失败，不修改状态。自动化建议使用 ID。`join`（也可写成 `select`）只选择项目，不会绑定目录。

```sh
cd /absolute/path/to/repository
clumsies project join AgentOS
clumsies project bind AgentOS
clumsies project current
```

`bind` 的目录默认是 `.`。目录属于其他项目时，错误会提供旧项目 ID 和实际绑定版本。先用 `project bindings 旧项目ID` 查看，再明确执行 `project bind AgentOS . --revision 已查看的版本号`。不要猜版本号；旧项目已删除时，仍可按 ID 查询本地绑定。Draft 的 `--status` 支持 `open`、`submitted`、`discarded`、`merged`，过滤发生在分页之前。

## 登录与目录绑定

使用已配置完成的 Server。远程地址必须为 HTTPS，本机开发允许 loopback HTTP。密码和 Token 不放在命令参数中，也不输出到 CLI 结果。

```sh
# 浏览器 OIDC 登录；无法自动打开浏览器时加 --no-browser。
clumsies login --server https://app.clumsies.ai
# 部署启用密码登录时，隐藏输入密码。
clumsies login --server https://app.clumsies.ai --username owner
# 自动化可用 --password-stdin 从标准输入读取密码。
clumsies project list
clumsies project join prj_example
clumsies project bind prj_example /absolute/path/to/repository
clumsies project current
clumsies agent enable claude-code
clumsies agent enable codex --host-binary /absolute/path/to/codex
```

Windows 上，`agent enable codex` 优先发现已注册的 Codex App，再查找 PATH；`--host-binary` 仍可指定宿主。MCP 程序及相邻 DLL 暂存到 `%USERPROFILE%\.clumsies\agent-runtimes\codex\<bundle-hash>`，让 Store 版 Codex 在被虚拟化的 AppData 之外访问它们。升级后重新执行 `agent enable codex` 并重连 Codex。旧暂存版本为正在运行的任务保留，会占用磁盘空间。


`project join` 在服务器确认成员权限后选择项目，不会授予成员权限；管理员需要先将账号加入该项目。`project create NAME` 按现有服务器权限创建项目。选择项目与绑定目录是两件事：Agent 根据工作目录解析绑定，不会回退到无关的当前项目。Git worktree 按 daemon 现有规则继承仓库绑定。

密码邀请使用 `clumsies redeem --server ORIGIN --username NAME`；密码重置使用 `clumsies redeem --server ORIGIN --reset-password`。命令隐藏输入一次性 Token 和新密码；`--stdin` 可读取含 `token`、`password` 的 JSON 对象，请保护这份输入。

浏览器登录复用现有 PKCE 与 loopback 回调，回调等待上限为五分钟。远程或无浏览器主机可使用部署已启用的密码登录，或通过 `--no-browser` 显示 URL，并将回调端口转发到浏览器所在机器。没有新增 device-code 登录协议。

适配器名称为 `codex`、`claude-code`、`opencode`、`dsh` 和 `antigravity`。宿主需要自行安装；Codex 可执行文件需要支持插件命令，找不到 PATH 入口时指定 `--host-binary`。`agent list` 查看状态，`agent disable HOST` 删除 daemon 管理的配置，并保护用户修改。绑定或适配器变化后重新连接集成或启动新的 Agent 任务。

先用 `project bindings PROJECT_ID` 查看绑定记录。替换绑定使用 `project bind ... --revision N`；解绑使用 `project unbind PATH --revision N`。它们校验已查看的版本，不删除仓库文件或本地 Draft。

## Draft、Review 与发布

Agent 使用现有 MCP `memory` 工具的 `activate`、`load`、`store`。`store` 先持久化本地 Draft，不直接发布。人工 Review 命令通过 daemon 的认证代理调用现有服务器接口。

```sh
clumsies draft list
clumsies draft show LOCAL_DRAFT_ID
clumsies draft sync LOCAL_DRAFT_ID
clumsies review create LOCAL_DRAFT_ID --title '完善发布流程'
# 多个本地 Draft ID 可以提交成一个有序 Review。
clumsies review list prj_example
clumsies review show REVIEW_ID
clumsies review diff REVIEW_ID
clumsies review comment REVIEW_ID --version 1 '已核对流程'
clumsies review approve REVIEW_ID --version 1 --note '可以发布'
clumsies review show REVIEW_ID
clumsies review merge REVIEW_ID --version 2 --reference CURRENT_COMMIT_ID
# 空引用用 ref-none。
clumsies review reject REVIEW_ID --version 1 --note '请修改提案'
```

上述版本号仅为示例，必须使用实际查看的版本和引用。服务器策略可能在批准时直接发布；返回 `merged` 时无需另行 merge。权限、版本、引用、批准内容指纹和生命周期规则仍由服务器判断。过期变更会失败，不会自动改用尚未审阅的新版本。拒绝后，作者可以修改 Draft 并新建 Review。

`review show` 返回完整有序操作及讨论；`review diff` 从每个 Draft 的不可变基础快照计算差异，不使用当前缓存冒充祖先。读取失败会终止，不会用空内容替代。

### 协调上游变化

初次提交前，`draft plan LOCAL_DRAFT_ID` 返回候选，其中含祖先、上游、提案、冲突和可编辑的合并预览。单独应用候选：

```sh
clumsies draft rebase LOCAL_DRAFT_ID --candidate CANDIDATE_ID --version DRAFT_VERSION --reference CURRENT_COMMIT_ID --resolved resolved-state.json
```

`resolved-state.json` 是用户编辑后的完整 `ReconciliationResourceState`，含 `exists`、`resource`、`content`。无冲突候选可不加 `--resolved`。也可在 `review create` 中用 `--reconciliations choices.json` 提交已查看的 `ReviewDraftRequest` 数组，包含 `draft_id`、`expected_draft_version`、`candidate_id` 和 `resolved_state`。同一个 Review 的 Draft 必须属于同一项目和 scope。

已有 Review 的更新必须包含全部提案与同一份一致版本：

```sh
clumsies review plan REVIEW_ID --version INSPECTED_VERSION > plan.json
# 查看 plan.candidates 和 plan.detail 中的全部提案。
# 将顶层 request 对象复制到 update.json。
# 有冲突时，编辑候选的 merge_preview.state，并填入对应 resolved_state。
clumsies review update REVIEW_ID --file update.json --reference CURRENT_COMMIT_ID
clumsies review diff REVIEW_ID
```

模板故意不填冲突结果，需要作者明确编辑后应用。服务器会拒绝过期候选、缺失提案、过期 Review 版本和已移动的引用。更新可能撤销原批准，需要重新查看和审阅。

## 诊断、升级与卸载

`status` 只检查，不启动 daemon。`daemon start` 复用兼容进程；Windows/Linux 的普通命令和 MCP 冷启动会按需启动 daemon。`status --project PROJECT_ID` 显示模型下载字节、准备状态、索引进度和错误。首次准备约 412 MiB 的固定模型；模型准备中会返回明确状态，不会伪装成成功的空搜索。

`draft sync` 最多等待一分钟完成上传，不丢弃本地工作。失败后先查看 `draft show`，再用 `draft retry PROJECT_ID` 重试。daemon 自动刷新过期 access token；refresh token 被撤销时重新执行 `login`，Draft 和绑定仍保留。`logout` 尝试服务器撤销，即使撤销失败也清除本地凭据。

升级时重新执行验证过的 Linux 脚本、Windows 脚本或安装器。安装流程协调启动锁、停止 resident、暂存整批程序，并在切换失败时恢复旧程序。入口路径保持稳定，已有适配器继续引用同一位置。Windows 提示文件占用时，先关闭 CLI/MCP 进程；升级后重新连接 Agent 宿主。

卸载保留凭据、绑定、Draft 和缓存；希望移除凭据时先 `logout`。降级前停止进程并备份完整 daemon 数据目录，不要让旧程序读取不兼容的新 schema。安装器不自动降级数据库，也不会覆盖无关程序目录。

源码构建使用 `cargo build --locked --release -p clumsiesd --bins`。CLI 没有另建凭据文件、缓存或业务规则。旧 Zig CLI 仍归档于 Git 提交 `4b18f7947a977dbc6b62f560b698dc992597f19d`，本次 Rust CLI 没有恢复它。
