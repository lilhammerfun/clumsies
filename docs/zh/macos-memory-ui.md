# macOS Memory 界面设计

本文描述 macOS App 当前已经实现的 Memory 工作区行为，以及仍未落地的交互缺口。底层
权威、Draft、Commit、Ref 和 Effective Memory 语义见
[《统一 Memory 模型》](./unified-memory-model.md)；Review 的有序多 Draft 契约见
[《macOS Reviews 当前设计》](./reviews-ui-design.md)。

## 1. 产品边界

Memory 工作区不是直接编辑共享权威的文件管理器。界面分为两个上下文：

| 上下文 | 当前展示 | 可执行操作 |
| --- | --- | --- |
| Organization | Organization Memory 权威目录 | 浏览、预览、把权威资源加入 Project；不创建或修改 Draft |
| Project | 当前 Project 选中的 Organization Memory、Project 携带的 Draft，以及兼容期的旧 Project 权威 | 通过 Project 绑定的 Draft 提议创建、编辑、改名或删除；移除 Project 选择；提交 Review |

Organization 视图没有明确的 Draft carrier，因此保持只读。Project 视图中的编辑也不会
原位修改 Organization 权威：选中的 Organization 资源与该 Project 的 LocalDraft 叠加后
形成可编辑文档。旧 Project-scoped authority 仍可见，但只作为兼容层展示，并带锁图标；
它不能通过当前发布链继续修改或提交。

界面仍以 Context、Rule、Workflow 等 kind 和路径约定组织创建入口，但这些只是统一
Memory 资源在线路模型上的分类与 UI 约定，不是彼此独立的存储系统。

## 2. 文件树与选择

文件树使用原生 `List(selection:)` 和 path tree：目录是从资源路径派生的节点，不是独立
Server 对象。首次出现时展开已有目录；资源或 Draft 路径变化后，树会重建并清理已经
失效的选中项。

- 单击文件选择并打开文档；目录点击切换展开状态；
- Command 和 Shift 支持多选与连续选择；
- 对目录执行操作时，目标集合是其下所有 terminal memory，而不是目录节点本身；
- 多选的 Open、删除提议、加入 Project 等动作复用同一目标集合；
- menu 是否出现和是否可用，同时取决于 Org/Project 上下文、权限、Draft 状态、freshness
  以及文档是否正在同步。

上下文菜单把普通文档操作和 Memory 领域操作分开：Open、Open Source、Rename Folder、
Delete Folder 属于文件树层；Add/Remove Project、Request Review、Discard Draft、Review
Shared Changes 属于领域层。删除共享资源始终创建 Organization deletion Draft，不在客户
端直接删除权威。

文件菜单使用 `Rename…` 和 `Delete…`，确认框说明修改先保存为草稿，审核合并后影响所有
引用该文件的项目。`Remove from Project` 只移除当前项目的引用。

文件名颜色表示尚未发布的新增、修改或删除。未提交草稿在右侧显示灰色 Draft 图标；
已提交草稿显示绿色 PR 图标，悬停显示 Review 标题，点击图标或选择 `View Review` 打开
对应审核。批量 Review 中的每个文件都能关联到同一审核；合并或放弃后清除草稿标记。

同步完成后仍有待审核修改时，状态提示为 `Synced · N changes in review`，并提供对应
Review 的入口。同步失败或数据过期时优先显示问题；同步本身不会提交或合并 Review。

### ZIP 导出

顶部导出按钮导出当前 Project 或 Organization 视图的全部记忆，不受搜索过滤影响。
文件树右键菜单支持单文件、多选和目录递归导出。目录导出包含被搜索隐藏的文件；文件与
目录混选时去重后放入同一个 ZIP。Memory Actions 菜单也可导出当前文件。
系统保存对话框选择 ZIP 的保存位置，完成后在 Finder 中显示。

ZIP 保留原有相对路径、文件扩展名和 UTF-8 正文，包含当前 Draft 的创建、改名、编辑以及
尚在等待自动保存的编辑文本；删除 Draft 不生成文件。导出捕获点击时的工作区内容，
不会发布草稿或自动更新到较新的共享版本。

尚未加载的正文复用现有版本与哈希校验；正文不可用、路径不安全或文件路径冲突时，整次
导出失败，避免悄悄漏文件。草稿清单未加载完成、文档正在同步时禁用导出。压缩在后台
调用 macOS 自带的 `ditto`，完整 ZIP 生成后才原子写入目标文件。

这里导出的是文件快照。管理员 `/api/v1/admin/memory-export` 仍提供包含 Draft 操作、
Project 选择和 Bundle 的迁移用 JSON。

## 3. Project 选择

Organization 视图可把单个、多个或一个目录下的资源加入任意 Project；Project 视图可把
已选中的 Organization 资源从当前 Project 移除。这项关系由 Server 的 Project Org
selection 权威维护，需要 `admin:write` 能力。

一次选择变更执行：

```text
GET 当前完整 selection 与 revision
  -> 在客户端计算完整 resource_id 集合
  -> PUT 完整集合，并用 If-Match 携带 revision
  -> 成功后更新本地 Project 投影并触发同步
```

因此一次 Add/Remove 请求是 Server 侧的整体替换和 CAS，而不是逐资源写入。请求失败、
权限不足或 revision 冲突时，不先行修改本地选择；重新读取权威状态后再重试。Project
移除一个干净的 Organization 资源后，对应 Project tab 会关闭；该资源仍存在于
Organization 权威中。

## 4. 创建、改名与删除提议

当前创建入口只存在于 Project 上下文，创建的是 Project-carried Organization Draft。
Organization 视图不提供“新建”；Project 中的旧 Project 权威也不能编辑。

单文件改名按两类处理：

- 已有 Organization 资源：在当前 Project 创建 rename proposal；
- 尚未发布的 create Draft：直接调整其提议路径。

文件夹改名先计算所有后代的新路径，拒绝空名称、`.`、`..`、斜杠、目录自包含和大小写
归一化后的路径冲突。文件夹删除会为已有 Organization 资源创建 deletion proposal，
并丢弃同目录下尚未发布的 create Draft；已经是 deletion Draft 的项不重复处理。

目录改名、目录删除和批量 Discard 在当前实现中仍是按文件顺序调用多个 mutation，并非
一个跨文件事务。某一步失败时，之前的步骤可能已经完成；界面显示 `Completed n of m`
一类进度和错误，调用方不能把批量操作理解为全成或全败。

## 5. 标题颜色与同步指示

文件名颜色只表达当前 Project Draft 的变更类型：

| 条件 | 标题颜色 |
| --- | --- |
| 没有 Draft | 系统 primary，包括继承自 Organization 的干净资源 |
| 新建 Draft（没有 target） | 绿色 |
| 更新或改名 Draft | 琥珀色 |
| 删除 Draft | 红色 |

Draft 类型优先于其他视觉状态。颜色不是唯一信号：删除、同步和只读状态还必须通过菜单、
图标、help 与 accessibility label 表达。

右侧同步 accessory 与标题颜色是正交信息，按以下优先级折叠：

1. Draft behind 且 reconciliation 有冲突：橙色警告三角，提示 shared update conflicts；
2. Draft behind 且权威资源正文或路径已变：灰色同步图标，提示文件的 shared version 已变；
3. Draft 只因 base Ref 落后：同一灰色同步图标，提示 Draft base behind；
4. 没有 behind Draft，但本地资源快照 stale：灰色同步图标，提示有更新的 shared version；
5. 其他情况不显示同步 accessory。

`freshness` 与 `hasUpstreamResourceChanges` 是两个事实：前者表示 Draft base 是否落后，
后者表示目标资源本身是否发生变化。不能只凭一个同步图标推断冲突。旧 Project 权威另用
灰色锁图标表示只读，不复用 Draft 颜色。

## 6. 文档会话与同步

同一个 Organization resource 可以同时出现在 Organization 目录和多个 Project overlay
中。可变编辑状态使用 `(project_id, item_id)` 作为 Project 会话键；Organization 权威
tab 的 Project 身份为空。因此打开相同 resource ID 的 Org 视图与 Project 视图不会共享
未保存正文、同步任务或 reconciliation 状态。

每个上下文内同一文档只保留一个 tab；Preview、Source 与 Diff 是同一个 tab 的模式，
不是三个独立文档。Draft baseline 不可用时强制进入 Diff。Project 取消选择资源时，仅当
该 Project 没有仍存活的 Draft，才清理对应 tab。

编辑保存进入本机 Draft/outbox，再由 daemon 与 Server 同步。资源 stale 或 Draft behind
时，工具栏提供同步或冲突处理入口。Memory 的冲突处理使用独立原生窗口，带关闭、最小化、
缩放及全屏控件；主窗口保持可用，关闭原文档 tab 也不丢失解决进度。窗口记住尺寸与位置，
比较区和结果区分别滚动，并可拖动分隔线调整空间。
Remote 与 Draft 的冲突片段并排显示，逐处选择后可编辑下方合并结果；路径和删除冲突也需
明确选择。Save to Draft 只保存草稿，不批准或发布。尚有未处理冲突时禁用保存，整份覆盖
位于次要菜单并需要确认。取消、红色关闭按钮及 Command-W 都确认未保存编辑；保存期间
阻止关闭。退出登录或退出应用也检查冲突窗口的未保存编辑。
正在同步的文档会锁住会改变路径、选择关系或 Draft 的操作。

## 7. Review 集成

Project-carried、open 且已同步到 Server 的 Organization Draft 可以进入 Review 提交
面板；freshness 为 behind 或存在冲突不会禁用入口。选中目录或多个文件时，客户端收集
所有符合条件的 Draft，用
`localizedStandardCompare(path)` 排序，并用一次请求创建一个有序多 Draft Review；
未变化文件和旧 Project 权威不会加入。该比较器没有额外 tie-breaker，因此不应把
当前顺序当成跨 locale 的规范化顺序。

如果任一 Draft behind，提交面板加载协调候选，有冲突的文件由用户逐项确认解决结果，
再在同一事务中协调全部草稿并创建 Review，不创建只覆盖部分文件的 Review。未同步、
缺少 Server ID、目录操作或文档同步进行中时，入口仍禁用。
Discard 和 deletion proposal 仍按各自 Draft 生命周期处理。Review 详情、评论和合并行为
由 [《macOS Reviews 当前设计》](./reviews-ui-design.md) 定义。

## 8. 状态恢复与可访问性

- 列表 selection、tab 和文档同步状态都以稳定资源/Draft 身份关联，不以标题或路径作唯一
  身份；
- 路径变化后刷新 tab 标题和文件树位置，失效上下文会被裁剪；
- 符号 accessory 带 `.help()` 和 accessibility label；颜色不承担唯一语义；
- 目录批量操作显示连续进度，失败信息保留已完成数量；
- selection、目录递归目标、菜单权限、tab 隔离、颜色和同步 accessory 均有纯逻辑或
  View 测试覆盖。

## 9. 当前缺口

以下是设计目标与当前实现之间仍然存在的差距，不应写成已交付行为：

- 干净的 inherited Organization 文件目前仍是 primary 颜色，也没有 `building.2` 徽标；
  “继承文件灰色并带作用域徽标”的旧视觉方案尚未实现。
- tab 标题目前只有文档标题，Preview 模式追加 `Preview`；没有 `Org ·` 或 Project 名称
  前缀。虽然会话已经隔离，同名跨作用域 tab 仍可能难以辨认。
- 即使目标 Project 只有一个，Add to Project 仍会打开二级菜单，没有单目标快捷动作。
- 新建命令只在文件树空白上下文出现，不能以当前选中目录作为创建位置。
- 文件夹 mutation 没有 Server 端批量事务，部分成功需要人工识别和重试。

## 10. 实现定位

| 关注点 | 当前代码 |
| --- | --- |
| 文件树、菜单、批量目录计划和视觉状态 | `apps/macos/Sources/Features/Memory/MemoryFileTreeView.swift`、`MemoryFileTree.swift` |
| Draft 与待保存编辑 | `apps/macos/Sources/Services/Memory/DraftStore.swift` |
| tab 与导航状态 | `apps/macos/Sources/Features/Workspace/WorkspaceNavigation.swift` |
| Project 切换与刷新编排 | `apps/macos/Sources/Features/Workspace/WorkspaceCoordinator.swift` |
| 文档 Sync 与共享刷新 | `apps/macos/Sources/Features/Memory/MemoryModel.swift`、`Services/Memory/MemorySyncService.swift` |
| 文档与会话模型 | `apps/macos/Sources/Libraries/Models/MemoryModels.swift` |
| 同步 accessory | `apps/macos/Sources/Libraries/UI/SharedUpdateIndicator.swift` |
| tab 标题与布局 | `apps/macos/Sources/Features/Memory/DocumentTabStrip.swift` |
| 交互测试 | `apps/macos/Tests/Features/FileTreeSelectionTests.swift`、`MemoryFileTreeMenuTests.swift`、`WorkspaceNavigationTests.swift` |
