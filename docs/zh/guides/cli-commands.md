---
title: Clumsies CLI
description: 安装 Windows/Linux 命令行，接入 Agent，并在没有图形客户端的情况下审阅和发布 Memory。
---
# Clumsies CLI

`clumsies` 是人工操作入口；`clumsiesd` 继续管理凭据、本地 Draft、缓存、目录绑定、同步和现有的 `clumsiesd mcp serve` Agent 入口。两个程序来自同一个 Rust 包，必须一起升级。Windows/Linux 图形客户端开发暂时搁置，相关实现和打包配置已从当前代码树移除；macOS 原生 App 继续维护。

## 安装

首批包面向 **Linux x86_64（Ubuntu 24.04 或兼容的 glibc 系统）** 和 **Windows x64**。已发布的压缩包和 Windows 用户安装器从 [GitHub Releases](https://github.com/lilhammerfun/clumsies/releases) 下载。尚未发布的改动可从仓库的 **CLI** 工作流下载已验证的构建产物。执行前，用旁边的 SHA-256 文件验证压缩包或安装器。

Linux 解压 `clumsies-cli-VERSION-linux-x86_64.tar.gz`，进入包目录执行：

```sh
sha256sum --check SHA256SUMS
./install.sh
```

安装完成后重新打开终端，再执行：

```sh
clumsies --version
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

macOS 安装 App 后，在 DMG 中双击 **Install CLI.command**（只有一个已安装 App 时自动发现）。也可明确指定 App：

```sh
sh /Volumes/Clumsies/Install\ CLI.command "$HOME/Applications/Clumsies.app"
# App 自带安装脚本，升级后也可重复执行：
sh "$HOME/Applications/Clumsies.app/Contents/Resources/install-cli.sh"
```

安装器创建 `~/.local/bin/clumsies`，自动向 bash/zsh 的用户启动配置追加 PATH，保留原配置。重新打开终端后直接执行 `clumsies --version`；无需手工 export。Linux 的 `install.sh` 同样负责 bash/zsh PATH。其他 shell 暂不支持自动配置，安装器会明确报错。安装器不会覆盖无关命令；macOS 入口指向 App 内嵌 CLI，原位置升级 App 后入口继续有效。移动 App 后需重新设置入口，不能继续使用旧路径。CLI 复用 App 的 launch agent、Keychain、配置和缓存；macOS 的 daemon 由 App 管理。

## 输出与阅读

默认输出可读文本：列表显示名称、ID、状态和关键同步信息，详情保留 Review 版本、发布引用、作用域及变更证据。`status` 简要显示准备情况和需要处理的问题；`--verbose` 显示诊断细节。长文本在交互终端中自动使用 pager，短输出直接显示。可用 `CLUMSIES_PAGER` 或 `PAGER` 选择阅读器，`--no-pager` 直接打印。默认探测 `less`，否则使用可用的 `more`（Windows 使用系统阅读器）；没有阅读器时直接输出。键位由阅读器决定。JSON、管道和重定向不启用 pager，不输出颜色或动画。

```sh
clumsies project list
clumsies draft list --status open
clumsies review diff REVIEW_ID
clumsies --no-pager review show REVIEW_ID
clumsies project list | grep AgentOS
```

**脚本迁移：以前默认输出 JSON，现在必须显式加 `--json`。** JSON 保留原有响应结构，避免混入人类提示。生成可编辑的协调文件也必须使用 JSON：

```sh
clumsies --json status
clumsies project list --json
clumsies review plan REVIEW_ID --version INSPECTED_VERSION --json > plan.json
```

## 自动分页与项目选择

文本模式的 `project list`、`review list PROJECT`、`review comments REVIEW_ID` 和 `draft list` 自动跟随服务端游标，逐批展示，无需手工复制 cursor。退出 pager 后停止后续取数，阅读器可能预读数据，已经进行中的一次请求可能完成。中途失败会明确提示列表不完整并返回非零退出码；已经显示的文本不代表完整结果。记录并发变化时，跨页结果也不是固定快照。

JSON 模式保留已有 `--limit`（每次请求 1–200 条，默认 100）、`--cursor` 和 `--all` 语义：默认一页；`--all` 从第一页读取全部结果，不能与 `--cursor` 同用。所有页成功后才输出合并 JSON；任一页失败或游标循环时不输出不完整结果。`--all` 的 JSON 会在内存中保留合并结果。文本模式下 `--limit` 仍是请求批量大小，不限制总条数；`--cursor` 指定起始位置，后续自动遍历。

```sh
clumsies project list --json --limit 20
clumsies project list --json --cursor '返回的游标'
clumsies review list AgentOS --json --all
```

旧 Server 可能忽略分页参数并最多返回 200 条；CLI 无法恢复服务器未提供的记录。Server 列表使用 `page_info.next_cursor`，本地 Draft 使用顶层 `next_cursor`。

项目 `show`、`join`（别名 `select`）、`bind`、`bindings` 和 `review list` 接受项目 ID 或唯一名称（忽略大小写）。同名时失败，自动化建议使用 ID。`select` 只选择项目，不绑定目录，也不授予权限。

```sh
cd /absolute/path/to/repository
clumsies project select AgentOS
clumsies project bind AgentOS
clumsies project current
```

`bind` 的目录默认是 `.`。目录属于其他项目时，先用 `project bindings 旧项目ID` 查看，再明确执行 `project bind AgentOS . --revision 已查看的版本号`。不要猜版本号；旧项目已删除时，仍可按 ID 查询本地绑定。Draft 的 `--status` 支持 `open`、`submitted`、`discarded`、`merged`，过滤发生在分页之前。

## 当前目录与 Draft 筛选

文本模式的 `draft list` 和省略项目参数的 `review list` 使用当前目录绑定的项目，不会退回全局选中的项目。未绑定目录或项目不可访问时明确失败：先绑定目录，或显式指定项目。`draft list --global` 查看所有本地项目保留的 Draft，包括不可访问项目。显式项目 ID 可以查询保留的本地 Draft，无需服务器成功解析项目名称。

```sh
cd /absolute/path/to/repository
clumsies draft list --status open
clumsies review list
clumsies draft list --project AgentOS --scope project --status open
clumsies draft list --global
```

Draft 的项目、作用域和状态筛选都在 daemon 分页之前执行。`--scope` 接受 `project` 或 `org`；`--project` 与 `--global` 互斥。JSON `draft list` 保留原来跨项目的默认行为，脚本需要指定项目时显式加 `--project`。CLI 和 daemon 必须一起升级；旧 resident 返回不符合筛选的 Draft 时，CLI 会报错，不会把未筛选的集合当作成功结果显示。

## 多行输入与编辑器

评论支持直接传文本、`--file PATH`、`--file -`（stdin）或 `--editor`。创建 Review 支持 `--description-file`；批准、拒绝支持 `--note-file`。这些命令的 `--editor` 分别编辑说明或决定理由。输入来源互斥；文件和 stdin 必须是 UTF-8，最多 4 MiB。空评论在提交前被拒绝。

```sh
clumsies review comment REVIEW_ID --version INSPECTED_VERSION --file comment.md
printf '第一行\n第二行\n' | clumsies review comment REVIEW_ID --version INSPECTED_VERSION --file -
clumsies review create DRAFT_ID --title '提案说明' --description-file description.md
clumsies review approve REVIEW_ID --version INSPECTED_VERSION --note-file decision.md
clumsies review comment REVIEW_ID --version INSPECTED_VERSION --editor
```

编辑器优先使用 `VISUAL`，其次 `EDITOR`（如 `code --wait`、`vim`，Windows 可用 `notepad`），必须等待编辑结束后才退出。编辑器模式要求 stdin 和 stdout 均为交互终端；自动化或重定向 JSON 回执使用文件/stdin。编辑器成功退出就提交输入；取消时让编辑器以非零状态退出，空评论也不会提交。编辑器、校验或服务器失败时，保留私有临时目录中的输入文件，并在 stderr 给出路径；提交成功后清理。保留文件可能含私有内容，恢复后请删除。

## 登录与目录绑定

使用已配置完成的 Server。**新用户必须先由管理员邀请；CLI 不提供开放注册。** 根据邀请方式选择下面一条路径。远程地址必须为 HTTPS，本机开发允许 loopback HTTP。密码和 Token 不放在命令参数中，也不输出到 CLI 结果。

### 已通过 Google 邮箱被邀请

```sh
clumsies login --server https://app.clumsies.ai
```

在浏览器中选择**被邀请的那个 Google 账号**（其他部署使用其配置的单点登录服务）。服务器确认成员资格后，完成首次激活和登录；未被邀请的邮箱不能自动加入组织。无法自动打开浏览器时加 `--no-browser`。

### 没有 Google 账号，使用邀请码

管理员提供一次性邀请码后，首次执行：

```sh
clumsies redeem --server https://app.clumsies.ai --username myname
```

将 `myname` 换成你要设置的用户名，按提示输入**邀请码和新密码**。成功后账号已激活，CLI **也已登录，无需再执行 `login`**。

以后登录已有的密码账号时执行：

```sh
clumsies login --server https://app.clumsies.ai --username myname
```

按提示输入密码；自动化可用 `--password-stdin` 从标准输入读取密码。未兑换的邀请不能直接通过密码登录完成注册。

### 两条路径登录成功后：选择项目并绑定目录

```sh
clumsies project list
clumsies project join prj_example
clumsies project bind prj_example /absolute/path/to/repository
clumsies project current
clumsies agent enable claude-code
clumsies agent enable codex --host-binary /absolute/path/to/codex
```

Windows 上，`agent enable codex` 优先发现已注册的 Codex App，再查找 PATH；`--host-binary` 仍可指定宿主。MCP 程序及相邻 DLL 暂存到 `%USERPROFILE%\.clumsies\agent-runtimes\codex\<bundle-hash>`，让 Store 版 Codex 在被虚拟化的 AppData 之外访问它们。升级后重新执行 `agent enable codex` 并重连 Codex。旧暂存版本为正在运行的任务保留，会占用磁盘空间。

`project join` 在服务器确认成员权限后选择项目，不会授予成员权限；管理员需要先将账号加入该项目。`project create NAME` 按现有服务器权限创建项目。选择项目与绑定目录是两件事：Agent 根据工作目录解析绑定，不会回退到无关的当前项目。Git worktree 按 daemon 现有规则继承仓库绑定。

密码重置使用 `clumsies redeem --server ORIGIN --reset-password`。命令隐藏输入一次性 Token 和新密码；邀请兑换或密码重置也可用 `--stdin` 读取含 `token`、`password` 的 JSON 对象，请保护这份输入。

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
clumsies review plan REVIEW_ID --version INSPECTED_VERSION --json > plan.json
# 查看 plan.candidates 和 plan.detail 中的全部提案。
# 将顶层 request 对象复制到 update.json。
# 有冲突时，编辑候选的 merge_preview.state，并填入对应 resolved_state。
clumsies review update REVIEW_ID --file update.json --reference CURRENT_COMMIT_ID
clumsies review diff REVIEW_ID
```

也可以直接在编辑器中更新：

```sh
clumsies review update REVIEW_ID --edit --version INSPECTED_VERSION --reference CURRENT_COMMIT_ID
```

编辑器显示 `request` 和完整 `plan`。查看候选及全部提案，将作者确认的完整状态填入 `request.drafts[].resolved_state`；冲突结果仍默认为 null。只有 `request` 会提交，编辑 `plan` 不会改变服务器证据。CLI 拒绝修改已查看的 Review 版本；候选、Draft 版本、完整提案集合及上游引用仍由服务器校验。JSON 模板保留路径、目录和作用域元数据，避免只编辑正文时丢失其他变更。`--file update.json` 和 `--file -` 仍接受原来的 request 对象。

模板故意不填冲突结果，需要作者明确编辑后应用。服务器会拒绝过期候选、缺失提案、过期 Review 版本和已移动的引用。更新可能撤销原批准，需要重新查看和审阅。

## 诊断、升级与卸载

`status` 只检查，不启动 daemon。`daemon start` 复用兼容进程；Windows/Linux 的普通命令和 MCP 冷启动会按需启动 daemon。`status --project PROJECT_ID` 显示模型下载字节、准备状态、索引进度和错误。首次准备约 412 MiB 的固定模型；模型准备中会返回明确状态，不会伪装成成功的空搜索。

`draft sync` 最多等待一分钟完成上传，不丢弃本地工作。失败后先查看 `draft show`，再用 `draft retry PROJECT_ID` 重试。daemon 自动刷新过期 access token；refresh token 被撤销时重新执行 `login`，Draft 和绑定仍保留。`logout` 尝试服务器撤销，即使撤销失败也清除本地凭据。

升级时重新执行验证过的 Linux 脚本、Windows 脚本或安装器。安装流程协调启动锁、停止 resident、暂存整批程序，并在切换失败时恢复旧程序。入口路径保持稳定，已有适配器继续引用同一位置。Windows 提示文件占用时，先关闭 CLI/MCP 进程；升级后重新连接 Agent 宿主。

卸载保留凭据、绑定、Draft 和缓存；希望移除凭据时先 `logout`。降级前停止进程并备份完整 daemon 数据目录，不要让旧程序读取不兼容的新 schema。安装器不自动降级数据库，也不会覆盖无关程序目录。

源码构建使用 `cargo build --locked --release -p clumsiesd --bins`。CLI 没有另建凭据文件、缓存或业务规则。旧 Zig CLI 仍归档于 Git 提交 `4b18f7947a977dbc6b62f560b698dc992597f19d`，本次 Rust CLI 没有恢复它。
