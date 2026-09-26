# Reviews UI 设计（macOS App）

Reviews 部分沿用原生工作区导航层级：全局应用侧栏保持原位，Review 列表是根页面，
选中的 Review 推入 `NavigationStack`。详情是文件审查工作区，
而不是常驻可见的第三个应用栏。

本文取代早先的网页式 Review 页面和临时的三栏 master-detail 变体。

## 1. 导航模型

外层壳有两个稳定角色：

```
global sidebar | NavigationStack (Review list -> Review detail)
```

- 进入 Reviews 打开列表，不自动选中某个 Review。
- 原生 `NavigationLink` 打开 Review。指针、键盘、
  VoiceOver 激活、Back 和导航转场行为都由系统负责。
- 从详情返回会恢复列表及其过滤上下文。
- 搜索和新建的 Review 可以 deep-link 到稳定的 `reviewId`。
- 全局侧栏按用户已有的侧栏偏好保持可见，不被 Review 导航替换。

## 2. Review 列表

使用原生 inset `List`，行内信息丰富并显示行分隔线。单击即推入 Review 详情。
滚动区域填满其内容区域，不绘制外边框；记录边界来自系统分隔线、焦点、选择、悬停和非活动窗口行为。
不要启用交替行背景：AppKit 会把条纹延伸到空表格区域，
使不存在的 Review 看起来像空行。分隔线的首端对齐固定在行上，不要让尾随作者列把它缩短。

列表是审查队列。每行回答五个浏览问题：改了什么、属于哪里、谁提交、何时更新、下一步需要处理什么。
行保持两行：

```
[lifecycle icon] review title [status]           [small avatar] author
project                                        <local date and time>
```

- 在作者下方、第二行右对齐显示固定的 Review 更新时间，不加文字前缀。
  tooltip 和无障碍标签说明这是最后记录更新时间，不是创建时间或实时计时器。
  详情头部也保留该更新时间。
- 完整描述留在详情页；列表不添加摘要。
- 把 Project 放在标题下方，作者用共享 `UserIdentityLabel`
  放在右侧，使用 20 pt 小头像（默认仍为 24 pt）。作者是提交者。
  左侧不重复他们的名字。长名字在尾随列内截断，并在 tooltip 中显示完整名字。
  按 Project 过滤或项目名不可用时省略 Project 副标题；
  绝不暴露不透明的 Project ID。
- 在标题前放一个生命周期图标。Open 使用 pull-request 图标，
  Merged 使用 merge 图标，Rejected 使用红色 pull-request
  图标。图标有无障碍状态名称和语义颜色，因此颜色从不是唯一信号。
  不要把 Open 或 Merged 重复为行内文字。
- 在 Review 列表中紧贴标题使用共享的紧凑 `InlineStatusBadge`，
  在文件导航行中右对齐使用。`Conflict` 使用系统红色；
  `Auto-rebased` 使用 GitHub 的 merged
  紫色（`#8250DF`）。两者都是白色 10 pt semibold 文字加胶囊，
  无边框或阴影。SwiftUI 原生 List badge 占据尾随位置，
  无法提供这种行内放置。冲突优先于 Review 中其他地方已完成的自动更新。
  `Checking…` 是瞬态，失败显示 `Retry Needed`。
- `Auto-rebased` 表示 Server 保存了干净结果，而不只是算出了预览。
  绝不要求用户保存自动 rebase。Merged Review 显示其生命周期，
  而不是 reconciliation badge。
  旧式 `Ready to Merge` 和作者重提操作仍按权限保留。
- 让 macOS 绘制分隔线、焦点、悬停/按下反馈和非活动窗口状态。不要绘制外层列表边框、
  逐行卡片或空白区斑马纹。`NavigationLink` 和 stack path
  是唯一的导航状态；不要添加并行的 `List(selection:)` 绑定，
  否则程序化 deep-link 时同一条路由可能被推入两次。
- 状态 Filter 菜单属于列表页的前置/导航工具栏区域。它折叠时的标签说明所选范围；
  计数仍保留在菜单、help 和无障碍值中。它默认 Open，包含 Open、
  Rejected、Merged 和 All 及计数。
  历史 Approved 记录仍可通过 All 访问；它们不再保留专用筛选器。
- 依次复用 Project、状态、作者的原生工具栏筛选菜单。这些筛选器与搜索组合；
  不需要额外的筛选行。
- Search 是独立的窗口级操作，并且始终是 Review 最右侧的工具。
  决策操作不与 Filter 分组；后台同步通知位于 Inbox。
- 没有缓存 Review 时的加载使用带标签的 `ProgressView`。
  刷新期间已有缓存行保持可见。空状态和过滤后为空使用
  `ContentUnavailableView`；搜索无结果使用原生搜索空状态。
  其他过滤后为空的状态包含 `Clear Filters`，它同时重置状态、作者、
  Project 和搜索。

GitHub pull-request 列表影响了信息顺序——标题第一，范围/作者/时间第二，
以及少量工作流信号——但不影响它的 Web chrome。不要复制蓝色链接、彩色药丸、
PR 编号、头像堆叠、评论计数或分页。Server 目前不提供未读数、
未解决线程数或真实的 last-activity 时间戳，
macOS 客户端不得从 `updatedAt` 编造它们。

## 3. Review 详情

推入的详情包含一个独立的拆分：

```
Review metadata + overall update status
changed-file navigator | selected file unified diff
```

文件导航器复用从 Memory 文件树提取的 path 层级、目录展开、文件符号和原生行样式。
它只负责 Review 文件选择；不得继承 Memory 的重命名、删除或打开副作用。

Review 的 `drafts[]` 元数据一到就构建树。
每个 Draft 贡献其最终文件 path；操作历史不会创建额外文件。
只在选中文件上加载快照内容并计算 diff，且在主线程之外。
在已加载的 Review revision 内共享已完成和进行中的 commit 请求。
文件加载和重试错误留在详情面板内，使导航器保持可用。
替换 Review revision 或离开页面会取消其加载器；迟到的响应不能替换当前选择。

commit 接口仍返回完整快照。它的首次下载是选中 diff 所必需的，但不阻塞文件树。
客户端日志分别记录目录就绪和单个文件加载耗时。

主面板只包含做决定所需的信息：

1. 标题和朴素状态；
2. 作者、Project 和更新时间；
3. 有描述时显示描述；
4. 有可操作的 stale/conflict 状态时显示；
5. 做出决定后的决策结果和审计元数据；
6. unified diff。

不要显示 `Changes` 标题或
`Create path · 20 changed lines` 之类的摘要。
文件导航器已经传达 path，diff 直接传达插入和删除。
仅删除和仅有元数据的 Review 保留一段简短明确的空状态，因为 diff 无法传达这些结果。

文件树标记落后文件和检测到的冲突。这些标记只描述单个文件；
选择当前文件绝不会把更新操作重定向到另一个文件。
整体 Review 头部和 stale 说明位于拆分上方。

作者直接在现有文件详情中审查远端改动。没有单独的 Review 更新窗口，
也没有 merged-result 编辑器或占位文字。同一个文件导航器包含当前文件、
自动 rebase 和冲突。

- 作为队列行或详情加载时，作者和组织管理员通过
  `POST /reviews/{id}/auto-rebases` 自动准备并保存干净的
  rebase。响应包含已保存的详情以及仍需要选择的冲突。
- 冲突文件把 Remote 和 Draft 并排显示为相对共同原版的 unified
  diff，选择按钮紧挨标题。选择某个 hunk 会保留该 hunk 之外的所有自动改动。
  path 和删除冲突有明确的选择；path 冲突可以输入自定义 path。
- 选择完成后，显示从最新远端状态到更新后 draft 的普通 diff。
  **File Actions (…) → Reset File Choices**
  恢复其选择。
- 自动 rebase 的文件使用普通 diff，
  并在其文件行右缘显示行内 `Auto-rebased` badge。
  状态来自当前 Draft revision 的持久化 rebase 历史，
  因此重新加载或重新打开 App 后仍保留。从未 rebase 的文件没有 badge。
  不添加状态行、文件计数或结果面板。
- **Review Actions (…) → Save Conflict
  Resolutions** 只对作者可编辑的冲突显示。它发送完整的有序提案集，
  只为冲突文件携带选择，并在每个冲突解决前保持禁用。version、
  membership 和 remote-reference 校验保持原子。这不会发布。
- 保存后刷新详情和文件标记；当前时移除保存操作，并只在当前详情可读后启用批准。
- 切换文件或 Review 保留未保存的选择。请求失败保留输入；
  **Check Latest Again** 在替换已编辑的解决结果前先确认。
  登出或退出会警告未保存的选择，并在保存期间阻止操作。
  账户/权限重置会清除选择并忽略迟到的响应。
- 仍有冲突的非作者会看到必须由作者解决。已有行评论保留在已保存的 revision 上；
  待处理的 reconciliation diff 在保存前不接受新的行锚点。

## 4. Diff 与评论

- replacement 行同时渲染删除和插入。
- 长行横向滚动。未变化区域保持折叠，除非展开或需要显示锚定评论。
- 行锚点是最终/新侧的 `(path, line)` 对。
  在 API 获得显式 side 字段前，删除行不能作为评论目标。
- 行 thread 紧接其准确 diff 行渲染，绝不移动到 diff 顶部。
- Review-wide 评论没有 path 或 line。它们位于用户显式打开、
  有明确标签的 `Review comments` 区域，且绝不能看起来像行内反馈。
- 针对更早 path 的锚定评论仍可在该区域以原 `path:line` 标签找到，
  而不是静默消失。
- 评论创建使用产生可见 diff 的那个 Review 详情版本。如果该版本已过期，
  严格的 Server 契约会拒绝它，详情会重新加载，而不是把评论锚定到无关内容。

在严格 Server 锚点部署之前创建的历史评论可能是 General，
因为旧 Server 在客户端解码其响应失败之前丢弃了未知的 path/line 字段。
客户端无法推断丢失的行；UI 诚实地标注它们，而不是假装它们是行内评论。

## 5. 工具栏决策

工具栏保留 Reject 和 Approve 为直接操作。Updates、
Merge 和 Resubmit 使用 **Review Actions (…
)** 中的明确文字条目，权限和就绪检查相同。**Save Conflict
Resolutions** 只在作者拥有的 Review 中已准备好的冲突时显示。
省略号保持在所属组的最后。加载、保存或等待冲突选择时它被禁用。该条目禁用时菜单仍可打开。
单个文件详情内没有单独的更新窗口按钮。

其他地方，Memory 导出操作共用现有的 **Memory Actions (…)** 菜单。
需要文字解释结果的操作应放入操作菜单，而不是给工具栏添加语义模糊的图标。

工具栏控件使用 `toolbarHelp` 注册原生 AppKit tooltip，
包括禁用的控件；文字说明操作名称并解释为何不可用。
系统生成的 Back 和 Sidebar 项获得其原生标签作为回退 tooltip。
侧栏提示跟随 Show/Hide 变化，但不覆盖显式 help。

决策操作保持菜单命令对等：

- Open：拥有 `review:decide` 和 `review:merge` 的
  Org owner/admin 会在标准工具栏样式中看到 Reject（`xmark`）
  和 Approve（`checkmark`）。
  Approve 在一个 Server 事务中记录决定并合并到 authority；
  普通成员仍是读取/评论参与者，看不到任何权限操作。
- Approved：历史记录在允许且 Server 提供了非空 approved
  result hash 时保留 **Review Actions (…
  ) → Merge Review**。没有该不可变结果身份的旧批准仍然可见，但不能合并。
- Rejected：Draft 作者可看到 **Review Actions (…
  ) → Resubmit Review**。

Filter 只属于列表页。决策工具只属于活动详情。Sync 保持自己的工具位。
Search 保持独立且在最右侧。跨区域工具栏分组和 macOS 14-26
放置方式作为独立的工作区级设计问题跟踪；在该工作完成前，
Reviews 不得重新引入一个万能操作组。

Reviews 隐藏冗余的全局 `In Review` 图标，同时保留同步进度、
失败和 stale 状态。

## 6. 状态与无障碍

| 状态 | 原生处理 |
| --- | --- |
| 加载 | `ProgressView` |
| 无 Review/无筛选匹配 | 上下文的 `ContentUnavailableView` |
| 详情加载失败 | 明确错误与 Retry；决策保持不可用 |
| Stale/conflict | 简洁的语义标签和就近操作 |
| 仅删除/仅元数据 | 主面板明确结果，而不是空 diff |
| 窄窗口 | 原生外层侧栏行为和 toolbar overflow |

- 保留系统焦点和链接激活反馈；不要只用颜色编码状态。
- 每个仅符号控件都有 `.help()` 和无障碍标签。
- 用鼠标、键盘和 Full Keyboard Access 验证 list ->
  detail -> Back。
- 验证嵌套 path、长 path、CJK 文本、长 diff 行、
  omission 内的评论、仅重命名的评论、stale 详情和加载/错误状态。

## 7. 数据边界

一个 Review 可以包含多条来自 Server 提供的 `drafts[]` 元数据的
draft。协调在其待审成员间聚合。更新规划覆盖每个 behind 成员；
应用完整有序集合是原子的。被丢弃的成员会分离，旧集合的批准失效。
如果 primary 成员被丢弃，下一个存续成员成为 primary。一个都不剩时，
Review 被拒绝，并保留最后一名成员作为历史。数据库迁移修复先前搁置的已丢弃成员关系；
客户端不得静默恢复已丢弃内容。客户端不从 draft 操作推断多文件 commit 历史。
旧式单数详情字段仍由当前客户端契约支持。
