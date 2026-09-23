# Project Memory 权威切换

本文记录从旧 Workspace 权威模型切换到 Organization Memory、Project 选择与 Draft overlay 的历史过程。

该历史目标已被 2026-09-23 确认的 [Project 与 Organization 归属决策](/zh/project-org-memory-ownership)替代，新模型通过新增迁移实现，并保留既有资源身份。历史切换操作不属于新模型的迁移步骤。

迁移后的单一事实链是：Organization Ref 指向 Commit，Project 选择资源，Project-carried Draft 构成私有 overlay，Review 合并后推进组织 Ref。旧 Workspace ID、双 ID 运行时分支和旧写入路径不再是现行合同。

当前字段和失败语义以[统一 Memory 模型](/zh/unified-memory-model)为准。
