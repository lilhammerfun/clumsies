# Project Memory 权威切换

日期：2026-08-26 · 迁移格式：version 1

这是历史上的「只保留 Organization」切换。
它的目标已被通过的
[Project 与 Organization 归属决策](/zh/project-org-memory-ownership)
取代，后者由保留既有身份的增量迁移实现。下面的命令执行的是旧切换，而不是新的迁移。

## 目标模型

Organization 是唯一活跃的 Memory 权威。Project 仍然负责仓库绑定、
成员关系、Organization Memory 选择、它的物化 Ref，
以及合并前 Draft overlay 的可见性边界。

因此一个新的、针对具体仓库的提案具有这样的身份：

```text
draft.project_id = <carrying Project>
draft.resource_scope = org
```

它并不通过把 `resources.scope` 从 `project` 改成 `org`
来表示。那样做会绕过 Review，并可能与既有的 Organization 身份或路径冲突。

## 旧模型转换

对每个包含活跃 Project 权威或活跃 Project 作用域 Draft 的
Project，迁移会：

1. 校验活跃资源正文、当前 Project Ref、
   已选 Organization 权威与 Draft 归属；
2. 按 daemon 顺序（`created_at`、Draft ID、操作序号）
   物化旧 Draft 操作，包括临时 ID；
3. 把该结果与所选 Organization 权威以及承载用户的活跃
   Organization Draft 合并；完全相同的旧路径会成为所选资源的更新提案，
   而该资源已有的活跃 Org Draft 会取代旧重复项并被上报；
4. 拒绝其余每种情况、前缀或文件/目录路径冲突；
5. 记录确定性的 plan hash 与 Effective Memory 路径/内容哈希；
6. 在 apply 时丢弃旧 Draft、归档 Project 资源、
   为每个保留的最终旧文件创建一个 Organization 作用域的
   create/update Draft，并只推进一个包含所选 Organization
   Memory 的 Project Ref。

当旧 Project Commit 仅在 description 上不同时，
活跃权威行中的 description 会被保留。任何身份、路径或正文漂移都是阻塞项。
活跃成员为零或多个的 Project 同样被阻塞，因为替代 Draft 的归属会有歧义。

apply 阶段是一个可串行化的 PostgreSQL 事务。
它会获取迁移与 Organization 协调锁、重新检查 Ref 与 Draft 版本、
拒绝过期的 plan hash、写入审计历史，并校验禁止活跃 Project 资源与活跃
Project Draft 的永久数据库约束。

## Runbook

先做 PostgreSQL 自定义格式备份，并保留受影响客户端的 SQLite 数据库。
在触碰生产之前，先针对还原出来的数据库演练这两条命令。

```bash
DATABASE_URL=postgres://... clumsies-server migrate-project-authority --dry-run
```

dry-run 会回滚它的规划事务，并且不运行 schema 迁移。审阅 `ready`、
`blockers`、每个 Project 的计数与警告，并保存返回的 `plan_hash`。

```bash
DATABASE_URL=postgres://... clumsies-server migrate-project-authority \
  --apply --expected-plan-hash 'sha256:...'
```

apply 会先安装待执行的 schema 迁移，
然后只有在实时 plan 仍具有审阅过的精确哈希时才继续。
失败的 apply 会回滚全部数据变更。成功之后再跑一次 dry-run，必须报告零旧权威、
零活跃旧 Draft 与零替代项。

apply 之后校验这些数据库不变量：

```sql
SELECT count(*) FROM resources
WHERE scope = 'project' AND status = 'active';

SELECT count(*) FROM drafts
WHERE resource_scope = 'project' AND status IN ('open', 'submitted');

SELECT conname, convalidated
FROM pg_constraint
WHERE conname IN (
  'resources_no_active_project_authority',
  'drafts_no_active_project_authority'
);
```

两个计数都必须为零，两个约束都必须已验证。保留备份，直到客户端已经同步了 discard 事件、
替代 Draft 与新的 Project Ref。此后 Desktop 的旧版锁附件会自然消失，
因为不再有活跃的 Project 作用域行；历史行的解析与丢弃仅为恢复兼容性保留。