---
title: 部署回滚清单
kind: procedure
scope: project
owner: platform-team
updated: 2026-09-25
---

# 部署回滚清单

当一次发布把错误版本带到线上时，按这个清单回滚。**先止血，再复盘。**

## 什么时候用

- 错误率连续 5 分钟高于基线
- 关键接口开始返回 5xx
- 数据写入出现不可逆错误

> 不确定的时候宁可先回滚：回滚的成本远低于继续观察的成本。

## 步骤

1. 确认当前线上版本号
2. 切换到上一个已验证版本
3. 验证健康检查
4. 通知相关同学

## 版本对照

| 环境 | 当前版本 | 回滚目标 | 负责人 |
| --- | --- | --- | --- |
| production | 2.14.0 | 2.13.3 | @lin |
| staging | 2.15.0-rc1 | 2.14.0 | @wang |

## 回滚命令

```sh
kubectl -n prod rollout undo deploy/api
kubectl -n prod rollout status deploy/api --timeout=120s
```

## 验证清单

- [x] 健康检查通过
- [ ] 错误率回到基线
- [ ] 补一条事故记录

相关文档：[发布流程](./release.md)，配置见 `docs/runtime.md`。

---

维护人：**platform-team**，变更请提交 Review。
