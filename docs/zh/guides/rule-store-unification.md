# Effective Memory 存储边界

本页取代已退役的客户端缓存统一提案。当前运行时只有一个由常驻 Rust daemon
拥有的读取模型；没有 CLI 缓存或 Agent 宿主 skill 目录是权威来源。

## 当前数据流

```text
Server Blob / Tree / Commit / Ref
  -> daemon validates and installs immutable Commit generations
  -> daemon applies current local Draft operations
  -> Effective Memory
  -> Project-local derived search revision
  -> clumsiesd MCP proxy forwards activate / load over XPC
```

`activate` 与 `load` 读取同一个 Effective Memory 快照。
一次成功的 `store` 会改变本地 Draft overlay，
从而改变 Effective Memory 哈希，
并入队一个匹配的增量 Index Revision。
先前就绪的 revision 在新 revision 原子发布之前保持可读。

## 所有权

| 数据 | 所有者 |
| --- | --- |
| canonical resource history and Refs | Server |
| cached immutable authority objects and local Ref pointers | resident daemon central SQLite |
| materialized Commit generations | Project Local Storage |
| current Drafts and queued operations | resident daemon central SQLite |
| complete Effective Memory resources, retrieval units, FTS rows, and vectors | Project-local search database |
| MCP parsing and response framing | short-lived `clumsiesd mcp serve` proxy |

proxy 不读取 generation 文件、不检查旧 manifest，
也不维护第二份资源缓存。轻量宿主原生 skill 层已退役：没有任何适配器安装 skill，
skill 目录也不是权威来源。

## 历史实现

先前的 Zig CLI 缓存、MCP 实现与 workflow-skill 生成代码仍可从
Git 提交
`4b18f7947a977dbc6b62f560b698dc992597f19d` 恢复。
它们在活跃构建、发布、安装与兼容边界之外。

当前契约见[本地运行时](/zh/runtime)、
[系统架构](/zh/architecture) 与 [MCP](/zh/mcp)。