# macOS Reviews 当前设计

Reviews 是 Organization Memory 发布链的授权界面。一个 Review **有序包含一个或多个
Draft**；Memory 文件树如何创建、选择和提交这些 Draft，见
[《macOS Memory 界面设计》](./macos-memory-ui.md)。

## 1. 导航模型

Reviews 使用统一的原生导航层级：

```text
全局侧栏 | NavigationStack（Review 列表 -> Review 详情）
```

- 进入 Reviews 默认显示列表，不自动选中第一条 Review；
- 每行使用 `NavigationLink`，点击、键盘和 VoiceOver 激活均由系统处理；
- 路由只保存稳定 `reviewId`，不把可过期的 Review model 放进 navigation path；
- Back 返回列表并保留过滤上下文；搜索或刚创建的 Review 可按 ID deep-link；
- Review 详情是主目的地，不用 sheet、popover 或常驻第三列承载长 diff。

## 2. Review 列表

列表使用原生 inset `List` 和系统分隔线，不绘制网页式卡片或空白区域斑马纹。
一行回答五个问题：改什么、属于哪个 Project、谁提交、何时更新、当前下一步是什么。

```text
[生命周期图标] Review 标题 [可选状态标记]     [小头像] <author>
<project>                                   Updated <本地日期和时分>
```

右侧作者复用 `UserIdentityLabel`，使用 20 pt 的 small 头像，其他调用默认仍为 24 pt。
长名字在右侧截断，tooltip 显示完整作者名。左侧不重复作者；已按项目过滤或项目名不可用时省略项目副标题。
固定更新时间在第二行右侧、作者下方显示，并以 Updated 明确这是记录更新时间，不是创建时间或实时计时。
详情页继续保留更新时间。

状态展示按以下优先级折叠为一个 signal：

1. `Merged`；
2. `Conflict`；
3. 保存成功的 `Auto-rebased`；检查中短暂显示 `Checking…`，失败显示 `Retry Needed`；
4. `Needs Review`；
5. 旧两阶段记录的 `Ready to Merge` / `Approved`；
6. `Resubmit` / `Awaiting Author`。

`Ready to Merge` 只适用于 status 为 Approved、当前用户可 merge 且 Server 返回非空
`approved_result_hash` 的历史记录。Merged 不因为 merge 后 Ref 前进而显示 stale。状态
必须同时有文字或 accessibility label，不能只靠颜色。

列表页复用原生工具栏筛选器，依次为 Project、状态、作者，Search 独立显示。Filter 提供 Open、Rejected、Merged、All 及各自计数；加载、空列表、过滤后为空和失败分别使用 `ProgressView` 或有
上下文的 `ContentUnavailableView`。后台刷新时已有行继续显示，失败 banner 提供重试。

当前列表不显示评论数、未读数、文件数或真实 last-activity：Server 没有这些可靠字段，
`updated_at` 只表示 Review 记录最近更新。时间使用固定的本地日期和时分，明确标注 Updated，不显示持续跳动的相对计时，也不把它称为创建时间。

## 3. 有序多 Draft 契约

创建和重提请求使用非空 `drafts[]`，每项包含 `draft_id` 与
`expected_draft_version`。Server 要求 Draft ID 不重复、由同一作者创建、属于同一 Project
和 authority scope、包含操作，并在提交前与当前 Ref 协调。更新请求必须包含完整、有序的 Draft 集合；每个 behind Draft 携带自己的 candidate，冲突项另带最终结果。

Server 的 `review_drafts(review_id, draft_id, ordinal)` 保存顺序并保证一个 Draft 最多属于
一个 Review。`Review.draft_ids[]` 与 `ReviewDetail.drafts[]` 按 `ordinal` 返回；Reject 会
重新打开全部 Draft，重提可更新有序集合但必须保持原 primary Draft 在首位。merge 在一
个 PostgreSQL 事务中按该顺序展开全部 Draft 操作，生成同一个 Commit，并将全部 Draft
置为 Merged。

macOS 从目录或多选发起 Review 时，先用 `localizedStandardCompare(path)` 排序，再发送
一个请求。详情为每个 `ReviewDraftDetail` 建立一项真实文件 change；不得把 Draft
operation history 伪装成额外文件。这里没有额外 tie-breaker，不能把顺序宣传为跨 locale
的规范化排序。

为兼容旧客户端，详情仍同时返回首项别名 `draft` / `operations`，列表仍有首项
`draft_id`。现行消费者应以复数 `drafts[]` / `draft_ids[]` 为准；macOS 仅在连接旧响应
缺少 `drafts[]` 时回退到单项别名。

## 4. Review 详情

详情内部使用两栏：

```text
Review 元数据 + 整体待更新状态
changed-file navigator | 当前文件 diff
```

文件导航器复用 Memory 的 path tree、目录展开和原生行样式，但只管理 Review 文件选择，
不继承 Memory 的重命名、删除、编辑或 Project selection 操作。每个 terminal 节点来自一
条 Draft 的最终 path，稳定 ID 优先使用资源 ID，没有资源 ID 时退回 Review/Draft 组合。

收到 Review 的 `drafts[]` 元数据后立即展示文件树。只有选中文件才加载快照并在后台计算
diff；同一 Review 版本内，共享已完成和进行中的 commit 请求。文件加载状态、失败与重试
都在详情区展示，目录仍可切换。Review 版本变化或离开页面时取消该加载器，迟到的响应
不能覆盖当前选中文件。

当前 commit 接口仍返回整份快照，首个 diff 需要等待这一次下载，文件树无需等待。
客户端日志分别记录目录就绪与单个文件的加载耗时。

主内容按顺序显示：

1. 标题和朴素状态；
2. 作者、Project、固定的最后更新时间；
3. 可选描述；
4. 有冲突时在当前文件详情中选择保留的内容；
5. 决策人、时间、说明与 immutable result hash；
6. 当前文件的 unified diff。

不要额外显示 `Changes` 标题或“20 changed lines”一类重复摘要。删除 Draft 显示明确的删除
结果；只有元数据变化而正文不变时显示对应空状态，不能让主面板看似加载失败。

Server 对待审成员返回聚合 coordination：任一 Draft behind 则 Review behind，任一
Draft conflicts 则 Review conflicts。文件树标记只描述各文件状态；选择已是最新的文件时，
不会再通过文件内按钮跳到另一个文件。整体状态位于文件分栏上方。

作者直接在现有文件详情中检查远端变化，不再打开独立 Review 更新窗口，也不显示
Merged Result 编辑区或混入正文的待选择占位符。文件树同时保留当前文件、自动合并文件和冲突文件。

1. 作者或组织管理员查看列表行或详情时，系统自动保存无冲突 rebase；返回保存后的详情及尚待处理的冲突。
2. 有冲突时并排显示 Remote 和 Draft 各自相对共同原版的 Diff，选择按钮与标题同行。
   选择只改变对应冲突片段，保留其他可自动合并的修改。路径和删除冲突明确显示选择；
   路径占用时可输入自定义路径。
3. 选择完成后直接显示“最新 Remote → 更新后 Draft”的普通 Diff，文件更多菜单的 **Reset File Choices** 允许重新选择。
4. 自动合并文件沿用普通 Diff。列表和文件树共用 `InlineStatusBadge`：外层列表紧贴标题，详情文件树按行靠右对齐。
   **Conflict** 使用系统红色，**Auto-rebased** 使用 GitHub merged 紫色（`#8250DF`），统一白色 10 pt 半粗字和小型胶囊，无描边或阴影。
   原生 List badge 固定在行尾，因此行内位置使用共享组件。`Auto-rebased` 来自当前
   Draft 版本已保存的无冲突 rebase 历史，重启后仍可读取，不能由候选预览推断。
   混合 Review 在列表中优先显示 Conflict，各文件分别显示实际状态；未 rebase 的当前文件不加标记。
   Diff 上方不增加状态行、数量统计或结果区。
5. 自动合并没有手动保存按钮。仅有作者可编辑的冲突时，提供
   **Review Actions (…) → Save Conflict Resolutions**，全部选择完成后可用。
   提交完整有序集合，仅冲突文件携带解决结果。Server 原子校验成员、版本、远端 Ref 和候选；
   失败保留选择。保存后刷新详情，当前且可读时启用批准。自动 rebase 和保存选择都不会批准或发布。

切换文件或 Review 保留未保存的选择。请求失败保留输入，Check Latest Again 在覆盖选择前
要求确认。退出应用或登出确认未保存选择，保存期间禁止退出。authority reset 清空状态并
拒绝迟到响应。非作者在仍有冲突时看到等待作者处理的说明。行评论仍锚定已保存版本；待保存 Diff 不接收新行评论。

丢弃成员时从待审集合移除并使原批准失效；丢弃 primary 后由下一存续成员接替。
最后一个成员也被丢弃时，Review 变为 Rejected，并保留最后成员作为历史记录。
数据库迁移修复旧的卡死成员关系，不恢复已丢弃内容。

## 5. Diff 与评论

- replacement 同时渲染 removal 和 insertion；长行横向滚动；
- 未变化区域保持折叠，除非展开或需要展示锚定评论；
- 行评论锚点是最终/新侧的 `(path, line)`，当前 API 没有 diff side 字段，删除侧行不能
  创建新锚点；
- Server 会在全部 Draft 的最终存续 path 中验证锚点，并验证新侧行号；
- inline thread 紧随准确 diff 行，不移到文件顶部；
- Review-wide 评论没有 path/line，放在用户显式打开的 `Review comments` 区；
- 旧 revision 或旧 path 的评论仍在该区以原 `path:line` 显示，不能静默丢失；
- 创建评论携带产生当前 diff 的 Review version。409 表示详情已变，客户端重新加载后再
  允许操作。

历史版本曾丢弃未知 anchor 字段，因此部分旧评论只能诚实显示为 General，客户端不能
猜测其原始行号。

## 6. 工具栏与权限

工具栏保留批准和拒绝为直接按钮。保存冲突选择、Merge、Resubmit 收入 **Review Actions (…)** 的文字菜单，
权限和就绪检查保持一致。Save Conflict Resolutions 仅作者有冲突候选时显示；加载、保存及仍有未解决冲突时
禁用该菜单项，菜单本身仍可打开查看操作名称。按钮保留 tooltip 和无障碍名称。
三点菜单始终放在所属工具栏组的最右侧。文件详情中不再提供打开额外更新窗口的按钮。

Memory 导出操作也收入现有 **Memory Actions (…)** 菜单。需要文字解释结果的操作放入菜单，
不再为它增加语义模糊的工具栏图标。

工具栏控件统一使用 `toolbarHelp` 注册 AppKit 原生 tooltip，禁用时仍显示操作名称和禁用原因。
系统生成的返回、侧栏按钮用原生标签补齐提示，侧栏提示随显示/隐藏状态更新，不覆盖显式提示。

决策动作：

| Review 状态 | 当前动作 |
| --- | --- |
| Open | 有 `review:decide` 的用户可 Reject；同时有 `review:decide` 与 `review:merge` 的用户可 Approve and Merge；按钮使用普通工具栏样式，不填充主题色 |
| Approved | 旧两阶段记录在 result hash 非空且有 `review:merge` 时可 Merge |
| Rejected | Draft 作者可 Resubmit |
| Merged | 无决策动作 |

Approve and Merge 直接调用 merge 路径，在一个 Server 事务中记录决定并前移 authority；
它不是先生成一个需要再次点击的 Approved 状态。所有决策都要求当前详情 readiness、当前
Review version 和当前 Ref，stale/conflict 时先协调。

Filter 只在列表页，决策只在详情页，Sync 和 Search 是独立工具位。符号按钮必须提供
`.help()`、accessibility label 和进行中状态。
Reviews 内不重复显示全局 `In Review` 图标；同步进行中、失败和落后提示仍保留。

## 7. 状态与验证

| 状态 | UI 行为 |
| --- | --- |
| 初次加载 | 标注用途的 `ProgressView` |
| 无 Review / 无过滤结果 | 对应 `ContentUnavailableView`，过滤空可 Show All |
| 详情失败 | 明确错误与 Retry；清除 decision readiness |
| stale / conflict | 无冲突自动保存；有冲突在详情处理并保存选择 |
| narrow window | 使用系统侧栏和 toolbar overflow，不自造响应式 Web chrome |

自动化覆盖路由只携带 ID、列表状态、工具栏 ownership、状态优先级、rendered-version
readiness、文件树、多 Draft 提交/顺序/merge、评论锚点和 stale 协调。人工验收还应覆盖
嵌套与超长 path、CJK、长 diff 行、omission 内评论、rename/delete-only 和 Full Keyboard
Access。

## 8. 已知限制

- Public OpenAPI 声明了 Review list 的 limit/cursor，但当前 HTTP 只读取 `project_id`，SQL
  固定最多返回 200 条且 `has_more` 恒为 false；调用方不能把它宣传为真实分页。
- 列表不显示文件数量；进入详情查看完整文件集合。
- 更新以整个 Review 为事务单位；完整 PR commit 时间线尚未实现。
- 评论锚点没有 old/new side，删除行只能作为 diff 内容查看。
- `ReviewDetail` 的单数兼容字段仍扩大了协议表面；移除前需要完成客户端版本迁移。
