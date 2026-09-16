# 工作目录绑定与 Memory 路由

MCP 代理根据自己的工作目录定位 Memory。宿主必须在当前任务的 workspace 中启动
`clumsiesd mcp serve`；全局 Plugin 的安装位置不决定 Project。

```text
MCP 进程 cwd → daemon 的 project_bindings → Project → Effective Memory
```

daemon 会规范化目录，在当前 Server 的绑定中匹配最近的祖先目录，并支持 Git worktree
解析到主仓库。未绑定目录返回 `project_binding_not_found`，不会回退到 App 当前选中的项目。

代理保存解析出的 Project，并自动填入 `activate`、`load`、`store` 请求；Agent 不提交
Project ID。每次工具调用都会复核启动目录的绑定。绑定改到另一个 Project 时返回
`project_binding_changed`，需要新建任务获取新的 MCP 连接。Shell 中的 `cd` 不会切换已有连接。

Codex 启动 Skill 和 MCP 工具说明负责提示何时使用 Memory。这条路径不依赖生命周期
Hook 或 AgentRun。Activity 直接读取宿主会话日志并关联 Retrieval Run。

## 从旧生命周期集成升级

适配器更新会清理已确认归 Clumsies 管理的生命周期脚本和注册项，保留其他工具的 Hook；
受管文件被修改时报告冲突。Codex Plugin 更新会移除旧 Hook 配置和脚本，重启 Codex 并
新建任务后加载新快照。

AgentRun 私有 IPC 和命令入口已移除。Schema 42 将旧记录保留在 `retired_agent_runs`
和 `retired_agent_run_events` 历史表中，运行时不再读写；新安装不创建这些表。
Retrieval Run 检索历史继续保留。
