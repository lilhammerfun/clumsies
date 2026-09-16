# 接口与参考资料

这里供你查阅领域操作、字段、权限和失败处理。按当前问题选择页面即可，无需从头顺序阅读。

想先上手使用，请跟随[快速开始](/zh/quickstart/)；想理解数据和接口为什么这样设计，请从[认识 Clumsies](/zh/overview)进入，再读架构和数据模型。

## 按问题查找

| 你的问题 | 阅读入口 |
| --- | --- |
| 这个能力属于哪个领域，应该调用哪类接口？ | [领域接口](/zh/reference/domain-api) |
| HTTP 请求长什么样，应该带哪个 version 或 ETag？ | [HTTP 契约与调用示例](/zh/reference/http-api) |
| Agent 怎样发现、加载和提出 Memory 变更？ | [MCP：Memory 工具](/zh/mcp) |
| 登录、角色、令牌刷新和本地凭据怎样工作？ | [认证与会话](/zh/reference/auth) |
| 项目里的术语是什么意思？ | [术语表](/zh/glossary) |
| Clumsies 会写哪些本地文件？ | [运行时文件与路径](/zh/runtime) |
| Coding Agent host 怎样启动本地集成？ | [Agent 适配器](/zh/adapter) |
| 对应的代码在哪里？ | [代码库地图](/zh/repos) |

## 机器可读的 HTTP 契约

| 契约 | 主要调用方 |
| --- | --- |
| [Public OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.public.v1.yaml) | 已认证的产品客户端，通常为 daemon |
| [Admin OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.admin.v1.yaml) | Organization Administration；同时描述健康检查和首次安装 |

仓库会校验 OpenAPI 路由清单与 Server 一致。部分已声明的分页和条件读取行为尚未在当前 handler 实现；接入前请读[契约边界](/zh/reference/http-api#contract-limits)。本地 XPC 和 MCP 有自己的可执行契约，不使用这两份 HTTP schema。
