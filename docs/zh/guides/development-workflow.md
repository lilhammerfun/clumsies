# 开发流程：Worktree

Clumsies 开发使用 Git worktree，让并发修改不共享工作树、
构建产物或开发运行时。同一会话同时只写一棵 worktree，后续任务默认沿用。
更换之前先收口或明确交接原有修改，并停止写入旧目录。新任务不自动要求新 worktree。

## 核心开发循环

1. 检查已有工作，合适时复用它的 worktree；
   需要新建时从主 checkout 和合适的基线创建：

    ```sh
    git worktree add .worktree/<name> -b codex/<name> main
    ```

2. 在该 worktree 内开发和验证。
3. 提交并合并一个聚焦的 PR。
4. 删除 worktree 前先停止它的 Dev Instance：

    ```sh
    just dev-macos-reset
    cd /absolute/path/to/main-checkout
    git worktree remove .worktree/<name>
    git branch -d codex/<name>
    ```

## 约定

| 项目 | 约定 |
|---|---|
| worktree | `.worktree/<slug>` |
| Agent 分支 | `codex/<slug>` |
| 提交标题 | `<area>: <summary>`，最多 72 个字符 |

无关改动放在不同分支。不要重写已推送且仍在审查的分支；
应新建分支并 cherry-pick 相关提交。

## Dev Instance 隔离

只改文档时构建并预览 VitePress，不需要 macOS 运行时。要运行产品时，
每棵 worktree 都可以用 `just dev-macos` 启动完整、
隔离的 Dev Instance。完整实例需要 Just、XcodeGen、Xcode、
Rust、Bun 和 Docker Desktop。
它的 canonical path 决定 App 身份、daemon 服务、运行时目录、
Keychain service、Compose project、
动态端口和实例专属 `CODEX_HOME`。本地 `just dev-macos` 在打开
App 前初始化 Server，并通过 daemon 完成 fake-OIDC owner
登录。登录失败即停止启动；测试者不应需要 setup 或登录页面。
远端 Preview 实例不适用。

```sh
just dev-macos-status
just dev-macos-logs
just dev-macos-down   # preserve instance data
just dev-macos-reset  # delete instance data and credentials
just dev-macos-preview path/to/preview.json
```

`down` 只停止该实例并保留数据。`reset` 删除其数据和测试凭据，
必须在删除 worktree 前运行。只有 `just install-macos`
可以替换稳定 Debug 安装。

### 可直接审查的测试实例

把 Review UI 构建交给他人测试时使用
`just dev-macos-reviews`。它完成本地 Server 初始化、
通过 daemon 登录 fake-OIDC owner、跳过 Agent 选择，
并在**青禾酒店**项目准备 12 个业务 Review。测试者不需要 setup code、
凭据或终端命令。重复运行会重新登录并保留已有 playground。

在 Dev App 中打开 **Reviews**，将项目筛选设为**青禾酒店**。标题、
描述和文档只包含酒店业务内容。例如 **两家门店早餐延长至十点** 提议把早餐从 09:00
延长到 10:00，而已发布的指南独立地把停车费从每天 30 元改成 50 元。
自动结果保留两项修改。案例编号和验收说明只保存在外部清单中，
绝不写进 Review description。案例覆盖当前文件与冲突文件混合、自动合并、
多段正文冲突、重命名冲突、任一侧删除、丢弃成员、可直接批准、已更新过的 Review、
被拒绝的 Review，以及同路径各自新增。在 **Rejected** 或 **All**
中查看 **周末早餐延长至十点半**。发布前先检查各案例：
发布会推进这个共享测试组织的 Remote，其他 Review 可能因此再次需要更新。

要测试编辑器打开期间更新变旧，先运行
`python3 dev/seed-review-playground.py --advance-remote`，
再应用旧结果。编辑器应保留输入并提供检查最新版本的入口。
实例的 `hotel-review-playground.json` 记录 case ID、
Review ID 和预期结果；其中没有凭据。早先的通用 playground
及其清单原样保留。这个构造命令只接受当前 worktree 的 loopback Local
实例，不接受 Preview 或生产 Server。

## 验证

| 层级 | 命令 |
|---|---|
| daemon | `cargo test -p clumsiesd --lib` 与 `cargo test -p clumsiesd --test daemon_lifecycle` |
| macOS App | `just test-macos` |
| Dev 生命周期 | `just test-dev-macos` |
| 公开文档 | `bun run build` |

### CI 选择与交付

每次 PR 和 `main` 推送都会运行变更识别、
workflow/策略检查和稳定的 `build` 汇总检查。
`dev/ci_impact.py` 从完整的 PR merge-base diff 或
push 范围选择额外任务，包含删除和重命名两侧。选择表记录在 Actions 运行摘要中。

| 改动层面 | 额外验证 | `main` CI 成功后的自动交付 |
|---|---|---|
| README 与仪表盘截图 | README/静态资源检查、文档构建、双语搜索 | 无 |
| `docs/`、`site/`、Bun 依赖 | 同样的文档检查 | 站点 |
| Server 源码与 migration | Server fmt/Clippy/文档/测试、daemon 集成测试、两种 Linux 镜像架构 | Server |
| daemon 源码或内嵌 Clumsies 插件 | daemon 测试、macOS 测试、XPC/Dev 生命周期、签名安装包、脚本 | 无 |
| macOS 源码与资源 | macOS 测试、签名安装包、脚本 | 无 |
| Server / macOS 测试文件 | 对应的测试任务 | 无 |
| 共用 Cargo manifest/lockfile | 全部 Rust/原生检查和 Server 镜像 | Server |
| 生产 Compose 或环境模板 | Server/镜像、daemon 集成、文档与脚本 | 先站点，后 Server |
| CI 策略、共享 action、未知路径 | 全部检查 | 两者 |

`.md` 后缀不代表文档：插件 skill 和 App 初始 Memory
是随程序交付的资源。Git 历史不足和手动运行 **CI** 都会选择全量检查。
`build` 在依赖失败后仍会运行；被选中的任务必须成功，只有明确未选中的任务才能跳过。

原生测试、XPC 集成和安装包验证并行运行。Rust 缓存只存依赖，
只有 `main` push 写入；Swift 缓存按 Xcode 和依赖声明保存包下载，
不保存 App 二进制。新的 PR 运行会取消被取代的运行；
每次 `main` 运行保留自己的验证和交付。

自动交付由这次 CI 运行只在 `build` 通过后调用，并使用它验证过的提交。
串行组件交付拒绝已被该组件更新改动取代的提交，但允许无关的中间提交。
只改 README 既不发布 Server 镜像也不部署站点。
手动 Server digest 重试/回滚和手动站点部署仍然可用；手动 CI 只做全量验证，
不部署。见
[组织部署](./deploy-for-an-org.md#github-delivery)。

本地运行策略回归检查：

```sh
python3 -m unittest discover -s dev -p 'test_ci_impact.py' -v
python3 dev/check-doc-assets.py --self-test
python3 dev/check-doc-assets.py
actionlint
```

## macOS 构建与打包

从仓库根目录运行这些命令。`just test-macos` 运行不依赖 Server
的常规测试套件；`just test-macos-live` 需要已运行且已认证的 Dev
Instance，并只使用该实例的 daemon 和 Server。

`just build-macos` 为当前 Mac 的架构在
`/private/tmp/clumsies-macos-build` 下创建未签名的
Release 构建。构建会把 `clumsiesd` 内嵌进 App bundle。
只有在已安装 daemon 的情况下迭代 UI 代码时才设置
`CLUMSIES_SKIP_DAEMON_BUILD=1`。

要更新长期 Debug 安装，运行 `just install-macos`。
App 在启动后协调全局 Codex Plugin；
重启 Codex 并新建任务才能使用更新后的插件。
Debug 构建使用 Xcode 的本地 ad-hoc 签名。内嵌 daemon 获得显式、
稳定的 designated requirement，
因此重建它不会让其 file-keychain access control 条目失效。

用以下命令重新生成 Xcode 工程：

```sh
xcodegen generate --spec apps/macos/project.yml
```

生成的 `Clumsies.xcodeproj` 被忽略；
只有它的 SwiftPM lockfile 纳入版本控制。

### 可下载的体验版

用 `distribution=preview`（手动运行的默认值）
和一致的 workflow/source ref 运行 **Release**
workflow。例如，合入 `main` 后：

```sh
gh workflow run release.yml --ref main -f distribution=preview -f ref=main
```

CI 构建 Apple Silicon App 和 daemon，对它们做 ad-hoc 签名，
创建并挂载 DMG，验证其内容、签名和架构，然后发布 tag 为
`macos-preview-<run-number>` 的 GitHub
pre-release，附带 DMG 和 SHA-256 校验和。
可下载的 Preview DMG 不需要 Apple 或 Sparkle 密钥。
当前 ONNX Runtime 依赖没有预编译的 `x86_64-apple-darwin`
库，因此不提供 Intel 体验包。它们不改变最新稳定版。

只有 Sparkle 找到可用更新后，用户账户旁的 **Update** 按钮才会出现。
关闭更新提示或完成一次更新流程会清除该按钮。Settings → General →
Check for Updates 仍可用于手动检查，并共用同一个更新器。
Sparkle 在 App 内下载并验证更新，然后 **Install and
Relaunch** 替换 App 并重启。账户菜单仍可通过点击头像或名字打开；
原来尾部的 chevron 已移除。

要通过这个流程提供 Preview 版本，请用与 App 的 `SUPublicEDKey`
匹配的密钥配置仓库 Actions secret `SPARKLE_PRIVATE_KEY`。
CI 为已有 DMG 签名，并把 `preview-appcast.xml` 发布到专用
`macos-updates` release。
Debug/Preview App 使用这个固定 feed，并在解压前验证归档签名。
缺少密钥时 DMG 发布仍然可用，既有更新 feed 不变；密钥无效时 feed 生成失败。
不会生成 informational／“Learn More” 更新。签名密钥由开发者维护一次，
不需要用户配置。

两个发布通道始终有有效 feed。如果尚未发布符合条件的更新，feed 没有版本条目，
Sparkle 报告没有更新。初始化空 feed 不需要签名密钥，也绝不覆盖已有 feed。
要在不发布 App 的情况下修复缺失的 feed URL，运行：

```sh
gh workflow run release.yml --ref main -f distribution=update-feeds -f ref=main
```

Preview 和 tag 发布也会初始化缺失的 feed，
并在发布符合条件的更新前验证公开 URL。

Release App 使用同一专用 release 上的 `appcast.xml`，
因此无关的 CLI 发布无法改写更新 feed。
仍在使用 `releases/latest/download/appcast.xml` 的现有
App 需要安装一次新的 Preview DMG。

体验版沿用 Debug runtime 契约，与 `just install-macos`
一样，使用常规 `ai.clumsies.desktop` App 身份和内嵌 daemon。
Release runtime 仍需要 Developer ID 签名才能安装 Agent；
ad-hoc 签名的 Release 构建无法通过该检查。
体验版不创建 Dev Instance。它没有经过 Apple 公证；
首次启动可能需要用户在 Privacy & Security 中放行，
受管理的 Mac 可能限制该操作。

### 分发签名

tag 发布构建 Developer ID 签名的通用 App，对它公证并装订票据，
验证 App 与内嵌 Agent runtime 具有预期的签名团队和
hardened-runtime 身份，然后创建签名并公证的 DMG 供下载。
GitHub Actions 发布
`Clumsies-<version>-macos-universal.dmg`、
Sparkle 签名的 ZIP 更新归档和 `appcast.xml`。
Sparkle 只扫描 ZIP，因此 DMG 不会为同一版本产生重复更新。
workflow 也可以从当前默认分支顶端以 `distribution=notarized`
手动触发，生成签名候选。手动候选同时包含 DMG 和 ZIP，
但不发布 GitHub Release 或 appcast。
tag 发布还会更新固定的 `macos-updates/appcast.xml` feed，
并显式标记为 Latest，供仍使用原 feed URL 的旧客户端使用。

DMG 包含 `Clumsies.app` 和 `Applications` 快捷方式。
用户把 App 拖入 Applications，推出磁盘映像，再打开已安装的 App。
ZIP 也可用于手动安装：解压后把 App 移到 Applications 再打开。
两种格式都不需要构建工具。Developer ID 签名和公证让 Gatekeeper
无需体验版的手动放行即可验证下载。改用 ZIP 不会取消这些检查。
这条分发路径不使用 App Store Review。

`just test-macos-package` 还会创建并挂载临时 DMG，
验证复制出的 App 签名和二进制，并检查仅 ZIP 的 appcast 选择。
这些本地检查使用 ad-hoc 签名，不向 Apple 提交。
可以用
`sh apps/macos/Scripts/create-dmg.sh /path/to/Clumsies.app /tmp/Clumsies.dmg`
从已有 App 创建本地打包预览；这条命令不会为分发包签名或公证。

把 Apple 证书、证书密码、公证账号、App 专用密码、Team ID、
临时 Keychain 密码和 Sparkle 私钥保存在受保护的
`macos-signing` GitHub environment 中，
使用 `release.yml` 引用的 secret 名称。
将该 environment 限制在默认分支和 release tag，
并要求在其 secret 暴露前经过审查。Sparkle 私钥只在 tag 发布时需要；
只有它的公钥提交在 `project.yml` 中。

Debug ad-hoc runtime 在安装边界被接受，
并支持纳管的 Coding Agent 集成。Release 包必须携带被接受的团队身份和
hardened-runtime 标志。
