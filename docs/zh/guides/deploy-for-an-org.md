# 为组织部署 clumsies

生产部署由 Rust Server、PostgreSQL、OIDC 配置和发布流水线组成。部署前必须固定镜像 revision，配置组织身份提供方、数据库迁移与密钥，并验证健康检查、登录回调和权限边界。

部署变更先在可恢复备份上验证 migration，再滚动发布。应用健康不等于 migration、后台投影或外部身份链路全部正常；这些边界需要分别检查。

具体环境变量和基础设施声明以当前仓库的部署配置为准，认证合同见[认证与会话](/zh/reference/auth)。

## CI 与自动交付

`main` 推送先按[开发流程的分层规则](/zh/guides/development-workflow#ci-分层与部署)运行检查。
固定的 `build` 汇总检查通过后，CI 才调用所需的 Server Delivery 或 Site Delivery，
并使用此次验证的同一个提交。README 和截图改动不会触发生产交付；Bun 依赖和站点部署脚本
改动会触发站点检查与交付。Server 镜像在 PR 中验证两种 Linux 架构，但不发布。

同一组件的交付串行执行。自动交付前再次核对 `main`：已有更新的组件改动时跳过旧提交，
无关文档提交不会阻挡 Server 更新。两种交付都需要执行时，先由站点交付同步共用的
Compose/Caddy 配置，再交付 Server。Server 仍按不可变镜像 digest 部署，并保留
`SERVER_AUTO_DEPLOY_ENABLED` 开关、生产环境权限和手动 digest 重试/回滚。
站点也保留手动部署入口；手动运行 CI 本身只验证，不部署。

密钥配置、首次安装和运维步骤见[完整部署指南](/guides/deploy-for-an-org#github-delivery)。
