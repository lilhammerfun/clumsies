# Metaprompt 移除

`META_PROMPT.md` 不再是 agent runtime 契约或检索语料的一部分。
MCP server 通过 `InitializeResult.instructions`
与每个工具描述传达稳定的协议指引。

被移除的 bootstrap 路径要求宿主会话 ID，
并在任何其他 memory 操作之前先做一次特殊的 `retrieve` 调用。
它把协议状态重复在适配器 hook、MCP 会话对象、
一个已退役的客户端事件日志以及一个特殊权威资源上。当前的 `activate`、
`load` 与 `store` 工具对该路径没有任何兼容分发。

该移除迁移会重写受影响的 Commit 链，使其 Tree 不含这个过时资源，
把每个依赖的 Ref 与 Draft base 推进到重写后的 Commit，
删除相关 Draft 与 Review，然后删除 Metaprompt 表。
daemon 的 schema 迁移会删除本地 Metaprompt Draft 与操作，
同时保留 Context、Rule 与 Workflow Draft。
由于 Server Commit ID 会改变，它还会重置可重建的 Commit 与检索缓存、
删除旧的物化 generation，并重放远端 Draft 投影，
使保留下来的 Draft 获得重写后的 base Commit ID。
迁移之后不留下任何兼容类型、端点或本地记录。

被移除的 bootstrap 内容是协议指令而非持久 Memory，因此它被删除而不是重新归类。
长期有效的行为约束、可复用流程与背景材料在统一模型中都是 Memory 资源；
没有任何东西被重建为单独的类型。

当前线上契约见 [MCP](/zh/mcp)，daemon 拥有的 Effective
Memory 路径见[本地运行时](/zh/runtime)。