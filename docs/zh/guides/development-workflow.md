# 开发流程：Worktree 与 Dev Instance

本文说明当前仓库的并行开发约束，以及 macOS 开发实例的真实隔离边界。Git worktree
只隔离源码和分支，不等同于运行时隔离。

## 1. 基本原则

同一会话同时只写一棵 worktree，后续任务默认沿用。确需更换时，先收口或明确交接原目录中的修改，停止对旧目录写入，再建立新工作区。不要把新任务自动等同于新 worktree。worktree 只隔离
源码和 Git 状态；App、daemon、端口、数据库、Keychain、缓存和日志仍可能互相冲突，
因此需要运行 macOS 产品时，应使用当前 worktree 的独立 **Dev Instance**。只改文档时运行文档构建和预览即可。

```text
worktree
  -> Dev App
  -> worktree 专属 resident daemon
  -> 本地 Server + PostgreSQL + fake OIDC
     或显式指定的远端 Preview Server
```

稳定 Debug 安装不是临时开发实例。日常 `dev-macos*` 命令不得替换稳定 App、daemon、
Application Support、Keychain 身份或全局 Codex Plugin；只有显式执行
`just install-macos` 才会更新长期 Debug 安装。

## 2. 核心开发循环

1. 在主 checkout 下确认当前工作区状态；没有需要复用的工作区时，从合适基线创建分支，例如：

   ```sh
   git worktree add .worktree/<name> -b codex/<name> main
   ```

2. 在新 worktree 内启动完整 Dev Instance：

   ```sh
   just dev-macos
   ```

3. 修改并运行覆盖所改层级的测试。
4. PR 合并且不再需要实例后，先在该 worktree 内清理实例，再回到主 checkout 删除它：

   ```sh
   just dev-macos-reset
   cd /absolute/path/to/main-checkout
   git worktree remove .worktree/<name>
   git branch -d codex/<name>
   ```

`reset` 会删除该实例的数据和测试凭据，不能用 `down` 代替。已经推送、仍需审查或尚未
确认合入 `main` 的分支不得通过 destructive reset 重写；需要拆分工作时应新建分支并
cherry-pick 相应提交。

## 3. 实例身份与所有权

`dev/dev-instance.sh` 对 worktree 的 canonical path 计算 SHA-256，并取前 12 位十六进制
作为 `instance_id`。这个身份贯穿：

- App bundle ID、产品名和显示名；
- daemon 与 Server LaunchAgent label、daemon Mach service；
- 实例根目录、Derived Data、daemon root/cache/logs；
- Keychain service；
- Docker Compose project 和动态端口；
- 实例专属 `CODEX_HOME`；
- `runtime.json` 中的构建、进程和资源所有权。

因此两棵 worktree 可以并行运行，而不会共享上述可变状态。生命周期脚本在每次操作前
校验 descriptor、canonical path、进程身份和所有权；`down`、`reset` 只处理当前
`instance_id` 声明的资源，并拒绝符号链接逃逸或身份不匹配的目录。

默认实例根目录是
`~/Library/Application Support/ai.clumsies.dev/instances/<instance_id>`。可用绝对路径
环境变量 `CLUMSIES_DEV_ROOT` 改变开发根目录，但不能让实例绕过所有权校验。

## 4. 两种运行模式

### 4.1 Local

`just dev-macos` 构建 App、App 内 daemon 和 Server，使用当前实例的 Compose project
启动 PostgreSQL 与 fake OIDC，初始化本地 Server，再启动专属 daemon 和 App。所有对外
端口动态分配且只绑定 loopback。

交付 Review 界面供人检查时，运行 `just dev-macos-reviews`。命令自动完成本地 Server
初始化、fake OIDC 登录和 daemon 凭据安装，跳过 Agent 选择，并在「青禾酒店」项目准备 12 个业务 Review。
检查者不需要填写 setup code、登录信息或执行终端命令。重复运行会重新登录并保留已有测试数据。

在 Dev App 打开 **Reviews**，将项目筛选设为「青禾酒店」。标题、描述及文件正文只写酒店业务内容。
例如「两家门店早餐延长至十点」提议将早餐结束时间从九点改成十点，已发布的入住指南则把停车费
从每天三十元改成五十元；自动结果应保留两项修改。案例编号和验收步骤仅保存在应用外的清单中，
不得填入 Review description。案例覆盖：混合文件状态、自动合并、
多段正文冲突、重命名冲突、双方分别删除、丢弃成员后审批、无需更新、已完成更新、拒绝后重新提交，
以及同路径各自新增。「周末早餐延长至十点半」在 **Rejected** 或 **All** 中查看。建议先浏览再批准发布；发布会推进
这个测试组织的 Remote 版本，因此其他 Review 可能再次需要更新。

检查“编辑期间 Remote 又更新”时，先打开更新编辑器，再由开发者执行
`python3 dev/seed-review-playground.py --advance-remote`。提交旧结果应被拒绝，保留编辑内容，
并提供检查最新版本的入口。实例目录下的 `hotel-review-playground.json` 记录案例编号、Review ID 和预期结果，
不保存凭据。旧通用案例及其清单原样保留。构造脚本只接受当前 worktree 的 loopback Local 实例，不能操作 Preview 或生产 Server。

### 4.2 Preview

`just dev-macos-preview <descriptor.json>` 仍运行当前 worktree 构建的 App 和完整 daemon，
但连接 descriptor 指定的远端 Server/OIDC，不启动本地 Server、PostgreSQL 或 fake
OIDC。descriptor schema 版本为 1，必须提供 `environment_id`、HTTPS `server_url` 和
未过期的 `expires_at`，可选 `oidc_issuer`。

脚本拒绝带凭据或额外 path/query/fragment 的 origin、已过期 descriptor，以及稳定生产
Server `app.clumsies.ai`；启动前还会检查 Preview 健康状态。同一实例若要切换到另一个
Preview 身份，必须先 `reset`。

Preview descriptor 只是连接凭据，不负责部署当前 worktree，也不证明远端运行的是未
提交源码。Preview 环境的创建、镜像发布和销毁属于外部 CI/基础设施职责。

## 5. 命令

| 命令 | 当前行为 |
| --- | --- |
| `just dev-macos` | 启动或复用当前 worktree 的 Local Dev Instance |
| `just dev-macos-reviews` | 自动初始化并登录 Local Dev App，准备 12 个 Review 交互场景 |
| `just dev-macos-preview <file>` | 用 Preview descriptor 启动当前 worktree App/daemon |
| `just dev-macos-status` | 校验 descriptor 并显示当前实例状态 |
| `just dev-macos-logs` | 查看当前实例的 App、daemon、Server/Compose 日志 |
| `just test-macos-live` | 通过已运行且通过身份校验的实例执行 live 测试 |
| `just dev-macos-down` | 停止当前实例，保留数据和凭据 |
| `just dev-macos-reset` | 停止并删除当前实例拥有的数据、容器卷和测试凭据 |
| `just test-dev-macos` | 测试实例身份、并行隔离、Preview 校验和 owned cleanup |
| `just install-macos` | 编译、安装并打开日常使用的 Debug App；不属于普通 worktree 循环 |

## 6. 验证矩阵

| 改动层级 | 至少运行 |
| --- | --- |
| daemon 库与生命周期 | `cargo test -p daemon --lib`、`cargo test -p daemon --test daemon_lifecycle` |
| macOS App | `just test-macos` |
| Dev Instance 脚本或身份 | `just test-dev-macos` |
| 已运行实例的端到端路径 | `just test-macos-live` |
| 公开文档 | `bun run build` |

CI 还会对 Dev Instance 脚本执行 ShellCheck、`just --dry-run` 和生命周期契约测试。Git
hooks 与 remote 由 worktree 共享；`clumsies-commit-format` 会检查提交标题不超过 72 个
字符，推荐采用 `<area>: <summary>`。

### CI 分层与部署

每次 PR 和 `main` 推送都会运行变更识别、workflow/策略检查和固定的 `build` 汇总检查。
`dev/ci_impact.py` 根据 PR 相对共同祖先的完整差异，或一次 push 的完整范围，选择额外任务；
删除和重命名前后的路径也参与判断。Actions 摘要会列出每层是否执行。

| 改动范围 | 额外验证 | `main` CI 通过后的自动交付 |
| --- | --- | --- |
| README、仪表盘截图 | README/静态资源、文档构建、中英文搜索 | 无 |
| `docs/`、`site/`、Bun 依赖 | 同上 | 站点 |
| Server 源码和 migration | Server fmt/Clippy/文档/测试、daemon 集成测试、两种 Linux 架构镜像 | Server |
| daemon 源码、内嵌 Clumsies 插件 | daemon 测试、macOS 测试、XPC/Dev 生命周期、签名安装包、脚本 | 无 |
| macOS 源码和资源 | macOS 测试、签名安装包、脚本 | 无 |
| Server / macOS 测试文件 | 对应测试任务 | 无 |
| 共用 Cargo manifest/lockfile | 全部 Rust/原生检查和 Server 镜像 | Server |
| 生产 Compose、环境变量模板 | Server/镜像、daemon 集成、文档和脚本 | 先站点，后 Server |
| CI 策略、共享 action、未识别路径 | 全部检查 | 两者 |

不能按 `.md` 后缀直接跳过：插件 skill 和 App 初始 Memory 会随程序交付。
Git 历史不足、手动运行 **CI** 时都执行全量检查。`build` 在依赖失败后仍会运行；
被选中的任务必须成功，只有明确未选中的任务才能跳过。

macOS 单测、XPC 集成和安装包验证并行运行。Rust 缓存只存依赖，只有 `main` push 写入；
Swift 缓存按 Xcode 和依赖声明保存包下载，不保存 App 二进制。新 PR 提交会取消旧的 CI；
每次 `main` 推送保留自己的验证和交付。

自动交付由通过 `build` 的 CI 调用，固定使用此次验证的提交。同一组件串行部署，
如果已有更新的组件改动，旧提交不再自动交付；后续无关改动不会阻挡已验证的更新。
只改 README 不会发布 Server 镜像或部署站点。手动 Server digest 重试/回滚、手动站点部署仍保留；
手动 CI 只做全量验证，不部署。

本地回归命令：

```sh
python3 -m unittest discover -s dev -p 'test_ci_impact.py' -v
python3 dev/check-doc-assets.py --self-test
python3 dev/check-doc-assets.py
actionlint
```

## 7. 当前边界

- `down` 有意保留状态，删除 worktree 前必须 `reset`，否则实例目录和凭据仍会存在。
- Preview 模式只消费既有 descriptor，不负责自动部署远端 Preview。
- 安全快照分支是否可删取决于其内容是否已进入 `main`，不能用“worktree 已删除”推断。

## 8. macOS 分发包

### 可下载的体验版

手动运行 **Release** 流水线，选择 `distribution=preview`（默认值），并让
workflow ref 与 source ref 一致。例如，合入 `main` 后执行：

```sh
gh workflow run release.yml --ref main -f distribution=preview -f ref=main
```

CI 构建包含 App 与 daemon 的 Apple Silicon 包、完成 ad-hoc 签名、生成并挂载 DMG，检查内容、
签名及架构，然后发布带有 DMG 和 SHA-256 校验文件的 GitHub 预发布版本，
tag 为 `macos-preview-<run-number>`。发布可下载的 DMG 仍不需要 Apple 或 Sparkle 密钥，
也不覆盖 latest 稳定版。当前 ONNX Runtime 依赖没有 `x86_64-apple-darwin`
预编译库，因此暂不提供 Intel 体验包。

只有 Sparkle 确认有可用更新后，侧栏底部头像右侧才显示 **Update** 按钮。
关闭更新提示或结束更新流程后，按钮会消失。Settings → General → Check for Updates
始终提供手动检查入口，与侧栏共用同一个更新器。Sparkle 在应用内下载并验签，用户点击
**Install and Relaunch** 后自动替换 App 并重启。头像或名字仍可打开账户菜单，
原来的右侧上下箭头不再显示。

要将体验版推送到应用内，开发者需在仓库 Actions Secret 中配置一次
`SPARKLE_PRIVATE_KEY`，且必须与 App 的 `SUPublicEDKey` 匹配；用户无需配置。
CI 为已有 DMG 签名，并将 `preview-appcast.xml` 发布到专用 `macos-updates`
Release。Debug／体验版使用此固定地址检查更新，下载包在解压前先验签。
未配置密钥时仍发布可下载的 DMG，保留原更新源；密钥错误时更新清单生成失败。
不再生成跳转下载页的 informational／Learn More 更新。

两个发布通道始终提供有效清单。尚未发布符合条件的新版本时，清单不包含版本条目，
Sparkle 正常返回“没有可用更新”。初始化空清单不需要签名密钥，也不会覆盖已有清单。
如需修复缺失的清单地址而不发布 App，可运行：

```sh
gh workflow run release.yml --ref main -f distribution=update-feeds -f ref=main
```

体验版和 tag 发布也会初始化缺失的清单，并验证公开地址可访问，再发布符合条件的更新。

Release 包使用同一专用 Release 中的 `appcast.xml`，避免旧 CLI Release 影响更新源。
仍使用旧 `releases/latest/download/appcast.xml` 的体验版需要手动安装一次新 DMG。

体验包沿用 `just install-macos` 的 Debug runtime 契约，使用正常的
`ai.clumsies.desktop` App 身份及内置 daemon，不创建 Dev Instance。Release runtime
仍要求 Developer ID 签名才能安装 Agent 适配器；直接给 Release 包做 ad-hoc 签名
会在此处失败。体验包尚未经过 Apple 公证，首次打开可能需要用户在“隐私与安全”中
手动放行；受管理的 Mac 可能不允许此操作。

### Developer ID 签名与公证

Release 流水线构建包含 daemon 的通用 App，完成 Developer ID 签名、公证和票据装订，
再生成同样经过签名、公证和装订的 DMG。GitHub Release 同时发布：

- `Clumsies-<version>-macos-universal.dmg`：用户下载后，将 `Clumsies.app` 拖入
  `Applications`，推出磁盘映像，再打开已安装的 App。
- 同名 `.zip`：用于 Sparkle 自动更新，也可手动解压后将 App 移入应用程序目录。
- `appcast.xml`：自动更新清单。生成时只扫描 ZIP，避免同版本 DMG 重复进入更新清单。

tag 发布还会更新 `macos-updates/appcast.xml` 固定地址，并明确设置为 Latest，
让仍使用旧更新地址的客户端也能找到正式版更新。

两种格式都不要求用户安装编译工具。通过官网或 GitHub 分发无需 App Store 人工审核，
Developer ID 签名和 Apple 公证可让 Gatekeeper 验证下载，无需体验版的手动放行；
换成 ZIP 不会省掉这些检查。

Apple 证书、证书密码、公证账号、App 专用密码、Team ID、临时 Keychain 密码和
Sparkle 私钥应配置在受保护的 `macos-signing` GitHub Environment，名称以
`.github/workflows/release.yml` 为准。手动选择 `distribution=notarized` 可从当前默认分支生成 DMG 和 ZIP
签名候选，不发布 Release 或更新清单；tag 发布还需要 Sparkle 私钥。

`just test-macos-package` 会创建并挂载临时 DMG，验证 App 签名、二进制完整性及
DMG／ZIP 更新清单选择与未签名更新拒绝。本地测试使用 ad-hoc 签名，不向 Apple 提交。已有 App 可通过
`sh apps/macos/Scripts/create-dmg.sh /path/to/Clumsies.app /tmp/Clumsies.dmg`
预览包装效果；这条命令不会替产物完成正式签名或公证。
