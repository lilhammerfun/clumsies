# macOS Memory 界面设计

macOS 的 Memory 界面编辑按 Project 归属的 Draft overlay，
同时把 Organization 权威显示为只读上下文。它不能绕过 Review，
也不能直接发布到 Organization Ref。

## 产品边界

- 文件树呈现所选 Project 的 Effective Memory。
- 打开一个资源时显示权威正文以及该项目承载的 Draft overlay。
- 创建、重命名、更新与删除操作都产生 Draft 提案。
- Review 发布到所选 Project 或 Organization 所有者。
  Project 内的编辑默认归 Project 所有；
  **Propose Organization Change…** 创建显式的 Org 提案。

## 状态与同步

Project 选择决定 Effective Memory、
Draft 与 Review 上下文。Project
或会话变化会在异步结果发布之前让陈旧的文档工作失效。
Desktop 与 MCP 使用同一个 daemon 与 Draft 契约；
revision 冲突、离线状态与同步失败保持可见，而不会表现为发布成功。

只有当选择仍然有效时界面才恢复它。加载、冲突、离线、选择与错误状态不只靠颜色表达，
键盘导航始终可用。

文件树保留文件类型图标与变更颜色：新增为绿色、修改为琥珀色、删除为红色，
已提交但尚未合并的改动同样如此。不再有任何操作的 Draft 记录不会把文件标为已变更、
不会覆盖已打开标签页中的已发布内容，也不会进入新的 Review 请求；
其记录仍保留用于历史与后续编辑。[Inbox](inbox.md) 汇集 Review 事件、
影响被引用 Memory 的远端共享更新与同步失败；全局侧栏显示其未读数。
后台状态不再在文件树尾部追加徽标或全局同步工具栏按钮。

文档没有额外的 Draft、Review 或远端更新状态条。**View Review**、
**Review Remote Changes** 与 **Update from
Remote Version** 仍在适用的既有文档与文件菜单中提供。
删除 Draft 会在文档内容中说明待处理的删除。冲突解决与保存失败停留在操作位置；
读取或归档通知不会发布、对账或丢弃内容。

文件菜单使用 **Rename…** 与 **Delete…**。
它们的确认框说明 Draft 与发布目标。Project 发布影响其成员；
Org 发布影响使用该 Org 资源的 Project。
**Remove from Project** 只移除该项目的引用。

## Review 请求

目录与多选 Review 请求会纳入属于同一个发布所有者的开放 Draft。
全 Project 请求汇集 Project Draft。Org 提案单独提交。
每个 Draft 都必须已同步并有 Server ID；目录操作与文档同步也会阻塞入口。

落后的 Draft（包括存在冲突的）仍可打开请求表单。干净改动会自动对账；
只有真正的冲突需要选择。与已发布版本一致的 Draft 会被保留但排除在 Review 之外。
如果没有剩余改动，表单会说明没有可审阅的内容。冲突解决结果与由此产生的 Review 一起提交。
入口不要求 freshness 是最新的。

请求表单可以在 Project 合并之后，把选中的 Project 条目记录为一次独立的
Org 贡献。Project Review 显示关联的 Org PR，或一个可重试的创建失败。
Org 的拒绝绝不会回滚 Project 的发布。

已验证的已发布更新会为未编辑的文件自动安装。编辑器中的未保存文本与 Draft 基线保持受保护；
干净的 Draft 自动对账，冲突的 Draft 通知其作者。

## ZIP 导出

顶部工具栏导出当前 Project 或 Organization 视图中的全部 Memory，
与搜索过滤无关。文件树右键菜单可导出单个文件、多个选中文件，或所选目录的全部后代，
包括被搜索隐藏的文件。文件与目录混合选择会去重为一个 ZIP。
Memory Actions 也可以导出当前打开的文件。

导出保留原始相对路径与 UTF-8 内容，包括本地 Draft 的重命名、编辑、
新建文件与编辑器中的未保存文本。删除 Draft 会被排除。
原生保存对话框选择 ZIP 的目标位置；Finder 会显示完成的归档。
导出使用已捕获的工作区视图，不发布改动，也不把它刷新到更新的远端版本。

未加载的正文走既有的、带版本与哈希校验的读取路径。正文缺失、
路径不安全或文件路径冲突会让导出失败，而不是漏掉文件或覆盖冲突条目。
加载 Draft 清单与正在进行的文档同步会阻塞导出。
压缩在主线程之外使用 macOS `ditto` 执行，并且只有在归档完整之后才替换目标位置。

这是文件快照。org-admin 的
`/api/v1/admin/memory-export` JSON 接口另行导出迁移状态，
包括 Draft 操作、选择与 Bundle。

## 实现边界

SwiftUI 位于 `apps/macos/` 下；Draft 持久化、
同步与权威校验位于共享 daemon 与 Server 契约中。计划中的交互作为缺口跟踪，
不作为已交付行为写入文档。

## 解决远端改动

Memory 冲突操作会打开一个独立的原生窗口，带标准的关闭、最小化与缩放/全屏控件。
主工作区保持可用；关闭来源标签页不会丢弃解决结果。窗口记住自己的位置与大小，
并使用与 Review 详情相同的冲突选项与结果 diff。

远端与 Draft 的冲突区块并排显示，形式是相对共同原始版本的统一 diff，
选择按钮位于各自区块的标题中。每次选择都会更新合并结果，而不会丢弃自动合并的改动。
完成选择后显示结果 diff，其菜单中包含 Reset File Choices。
路径与删除冲突需要显式选择；即使文件正文相同，路径冲突也会得到说明。

在所有选择完成之前，Save to Draft 保持禁用；它不会批准或发布。取消、
关闭按钮与 Command-W 会在输入被编辑过时确认；保存进行中会阻止关闭。
退出登录或退出应用时，也会先检查这些窗口，然后才丢弃未保存的编辑。