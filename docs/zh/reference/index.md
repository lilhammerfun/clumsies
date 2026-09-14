# 接口与参考资料

这里把产品操作与具体数据、接口对应起来。如果你刚接触 Clumsies，先读[项目概览](/zh/overview)、[架构](/zh/architecture)、[数据模型](/zh/data-model)和[核心流程](/zh/flows)，再回来查字段、权限和失败处理。

## 按问题查找

| 你的问题 | 阅读入口 |
| --- | --- |
| 这个能力属于哪个领域，应该调用哪类接口？ | [领域接口](/zh/reference/domain-api) |
| HTTP 请求长什么样，应该带哪个 version 或 ETag？ | [HTTP 契约与调用示例](/zh/reference/http-api) |
| Agent 怎样发现、加载和提出 Memory 变更？ | [MCP：Memory 工具](/zh/mcp) |
| 登录、角色、令牌刷新和本地凭据怎样工作？ | [认证与会话](/zh/reference/auth) |
| 项目里的术语是什么意思？ | [术语表](/zh/glossary) |
| Clumsies 会写哪些本地文件？ | [运行时文件与路径](/zh/runtime) |
| Coding Agent host 怎样启动本地集成？ | [Agent runtime](/zh/guides/agent-runtime) |
| 对应的代码在哪里？ | [代码库地图](/zh/repos) |

## 机器可读的 HTTP 契约

| 契约 | 主要调用方 |
| --- | --- |
| [Public OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.public.v1.yaml) | 已认证的产品客户端，通常为 daemon |
| [Admin OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.admin.v1.yaml) | Organization Administration；同时描述健康检查和首次安装 |

仓库会校验 OpenAPI 路由清单与 Server 一致。部分已声明的分页和条件读取行为尚未在当前 handler 实现；接入前请读[契约边界](/zh/reference/http-api#contract-limits)。本地 XPC 和 MCP 有自己的可执行契约，不使用这两份 HTTP schema。
