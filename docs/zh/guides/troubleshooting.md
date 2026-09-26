# 排查问题

本页帮助你先判断问题发生在哪一层，再选择恢复动作。无需先理解所有内部实现；
先记住[三种成功状态](/zh/flows)：本地保存、同步到 Server、授权发布。

## 从看到的现象开始

| 现象 | 先检查什么 | 下一步 |
| --- | --- | --- |
| Agent 提示找不到 Project | 当前任务目录、Project 目录绑定、所用 Server 是否一致 | 在 Desktop 确认绑定，重新启动 Agent 任务；见[Agent 接入](/zh/guides/agent-runtime) |
| 更新后 Agent 无法连接 | proxy 和常驻 daemon 是否来自同一 App 版本 | 完成 App 更新并重启相关进程，重新开启任务；见[宿主适配](/zh/adapter) |
| Draft 保存成功，但一直没有同步 | Draft/Project sync 状态、网络、Server 地址和登录状态 | 修复连接或重新登录，再用产品提供的 Retry；已保存的操作仍在本地队列 |
| Draft 显示 behind | 上游版本已前进，Draft Base 是否仍是旧版本 | 查看 Base / Current / Draft Result，确认协调结果；不要反复提交旧候选 |
| Review 提交或合并报版本错误 | Review/Draft 版本、候选和目标 Ref 是否过期 | 刷新最新状态，重新检查差异，再确认操作 |
| 内容已发布，Agent 仍看到旧内容 | Project 是否选择了该资源、本机 Commit 同步与索引是否就绪、是否有未合并 Draft 覆盖它 | 依次检查选择、同步、索引和 Draft；组织发布不等于所有设备已完成准备 |
| `load` 找得到，`activate` 没返回 | 任务 query、排名、预算和增量状态 | 查看该次 Retrieval Run；检索没有返回不代表资源不存在 |
| 自定义磁盘断开后检索失败 | Project Local Storage 的位置是否可访问 | 恢复磁盘访问，查看 storage 状态；不要删除中心数据库作为恢复手段 |
| Server 健康检查正常，页面仍很慢 | 具体请求、返回体大小、请求数和本地展示阶段 | 按下面的分段方法记录；health 成功不能证明业务请求或页面就绪 |

## 页面加载慢时怎样定位

以 Review 页面为例，至少区分以下时间：

1. 提交 Review 的 HTTP 请求开始和结束。
2. 读取 Review 详情的请求开始和结束。
3. 页面需要的 Commit 快照下载次数、是否重复获取同一 Commit、总字节数。
4. 本地解析、差异计算和页面展示完成。

如果第 1 步已经成功，就先读取已创建的 Review 状态，
避免因为页面尚未显示而重复创建提案。连续的短请求也可能积累成长等待；
单看最慢的一次请求会漏掉串行等待和重复下载。性能分析方式见[延迟模型与诊断](/zh/perfo
rmance/latency-model)。

## 提供哪些证据最有用

报告操作发生的时间与时区、App/daemon 版本、所在 Project、具体动作、
界面错误码，以及“最后一个确认成功的阶段”。网络错误若有 request ID，
保留它供客户端和 Server 日志关联；不要在问题描述中粘贴 token 或完整私有 Memo
ry 正文。

稳定安装默认把客户端诊断日志放在：

```text
~/Library/Logs/ai.clumsies/
```

具体日志文件、轮转和请求关联说明见[本地运行时](/zh/runtime)。
独立开发实例有自己的运行目录，应使用 `just dev-macos-logs`，
避免读错稳定版日志。系统崩溃报告位于 `~/Library/Logs/DiagnosticRep
orts/`；它适合诊断进程退出，不能替代业务请求日志。

## 缓存与编辑不能一起清掉

Project 的 generation 和搜索索引是可重建数据；
中心 SQLite 还保存尚未上传的 Draft 和操作队列。遇到缓存或磁盘故障时，
使用产品提供的 Project 缓存管理入口，并先确认错误属于该层。
删除 `local.db` 可能丢失尚未同步的编辑。

管理员检查部署、OIDC、数据库与服务健康时，继续读[部署指南](/zh/guides/depl
oy-for-an-org)和[认证与会话](/zh/reference/auth)。
开发者需要具体实现时，从[代码库地图](/zh/repos)按问题所在层定位。
## Project 移除后同步暂停

访问已移除或不可访问的 Project 时，旧版客户端实际上会收到 HTTP 404，
却报出 “Commit state response is missing ETag”。
请把 macOS App 与其内置 daemon 一起更新；
这个修复不需要 Server 侧的迁移。

更新后的客户端会在核对当前成员关系后暂停不可用的 Project，
其他 Project 与 Organization Memory 继续同步。
打开 **Inbox → Project sync paused → Manage Unavailable Projects**，
即可移除过期的目录绑定，或把保留在本地的 Draft 导出为 JSON。
移除绑定会保留仓库与 Draft。
受管的 Agent 集成必须先成功移除，才能移除该绑定；如果它们的目录不可用，先恢复其位置。
恢复访问后使用 **Check Again**。网络或登录失败会单独报告，不会移除本地绑定。
