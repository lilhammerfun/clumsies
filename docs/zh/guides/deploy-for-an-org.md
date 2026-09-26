# 为组织部署 clumsies

本指南部署一个自托管的 Clumsies 组织。
Rust Server 镜像只包含 API 与 OIDC 端点；
安装与管理在 macOS App 中完成。组织 Memory 是 Desktop 中 Memor
y 分区的 organization 范围，不是一个部署进程。

## 运行时边界

生产环境运行三个容器：

- PostgreSQL 保存全部权威、Draft、Review、身份、审计、Blob、Tree、
  Commit 与 Ref；
- Server 运行 migration、Public API、bearer Admin API、
  公开健康检查、Memory 导出与 OIDC；
- Caddy 终止公网 HTTPS 并反向代理 Server。

只有 Caddy 对外发布主机端口。Server 与 PostgreSQL 留在 Compose
网络内。

## 前置条件

- Docker Engine 与 Docker Compose v2 或更新版本；
- 一个公网 HTTPS 主机名；
- 一个 OIDC 机密客户端，并在 IdP 注册下面的回调地址；
- 一个按 digest 固定、已发布的 Clumsies Server 镜像。

```text
https://memory.example.com/login/oauth2/code/oidc
```

生产环境不得使用旧的 Python `docker-compose` 命令。
发布脚本要求 Compose v2 或更新版本，并拒绝所有更早的大版本。

## 配置

把 `.env.example` 复制为 `.env`，限制为安装管理员可读，
并替换每一个占位符。尤其是：

```dotenv
CLUMSIES_SERVER_IMAGE=ghcr.io/lilhammerfun/clumsies-server@sha256:published-digest
CLUMSIES_PUBLIC_ORIGIN=https://memory.example.com
CLUMSIES_DB_PASSWORD=replace-with-a-random-password
CLUMSIES_SETUP_CODE=replace-with-at-least-32-random-characters
CLUMSIES_OIDC_ISSUER=https://identity.example.com
CLUMSIES_OIDC_CLIENT_ID=replace-with-oidc-client-id
CLUMSIES_OIDC_CLIENT_SECRET=replace-with-oidc-client-secret
CLUMSIES_CLIENT_REDIRECT_URIS=http://127.0.0.1/callback
```

`CLUMSIES_CLIENT_REDIRECT_URIS` 是 Clumsies 客户端经身
份提供方之后的允许列表。loopback 模板只在确切的回调路径上接受 Desktop 的动态端
口。Server 从 `CLUMSIES_PUBLIC_ORIGIN` 推导 IdP 回调。
原生 App 直接调用 Server，不提供浏览器界面，因此部署不需要配置 CORS 来源。

当主机需要出网代理时，配置 Docker Engine 与 `.env` 中的标准代理变量。
镜像内不内嵌任何部署专用的代理或镜像站地址。

## 启动与初始化

```bash
docker compose --project-name clumsies -f compose.production.yml up -d --wait
curl --fail --silent https://memory.example.com/api/v1/admin/health
```

打开 macOS App，把 `https://memory.example.com` 填为 S
erver 地址。App 只接受 HTTPS 来源形式的远程地址，并会识别出需要初始化。
输入一次性 Setup Code、组织名称、默认 Project 与可选的邮箱域名，
然后在系统浏览器里完成 OIDC。App 与 Server 使用 state 加 `S256`
PKCE；Server 在一个事务中创建组织、首位 Owner、默认 Project、
外部身份与初始 Ref，之后才返回客户端授权码。App 用它换取 bearer token、
安装进 daemon，安装随即永久锁定。初始化完成后，
从生效的 `.env` 中删除 `CLUMSIES_SETUP_CODE`。

后续的组织配置、成员、Project、token、
审计与健康都在 App 的 **Administration** 分区管理。
Server 的 Admin API 只接受 bearer；
`/api/v1/admin/health` 保留用于部署诊断，
`/api/v1/admin/memory-export` 保留为需要认证的迁移导出。

如果 daemon 启动失败，在 App 中选择 **Administrator Recover
y**。它会直接登录同一个受信任的 Server 来源，并只在 App 内存里保留临时会话，
让管理员先检查健康、修复成员访问或吊销 token，再重试正常启动。

## GitHub 交付

`CI` 在 `main` 上的 `build` 汇总检查通过后，
仅当改动影响 Server 交付时才调用 `.github/workflows/server-d
elivery.yml`。这个可复用工作流为 `linux/amd64` 与 `linux/ar
m64` 构建经过验证的那个精确提交，带 OCI source/revision 标签与 pro
venance 发布到 GHCR，并部署 manifest digest。
同样的镜像架构在 PR 中只做构建检查、不发布。

交付串行执行；如果 `main` 上已经有更新的 Server 改动，就会拒绝该提交。
中间夹带的无关文档改动不会丢弃待发布的 Server 更新。
站点交付与 Server 交付都被选中时，先由站点交付同步两者共用的 Compose/Caddy
配置。手动触发只接受已存在的不可变 digest 及其完整提交，用于重试或回滚。

站点交付构建文档站与官网，同步到 `/srv/docs` 与 `/srv/www`，
复制 Compose 文件与 `deploy/Caddyfile`，
并 reload Caddy，使同步过来的配置在运行中的容器上生效。

GHCR 包通过 OCI source 标签与本仓库关联。把该包设为公开一次，
自托管安装即可无需个人 token 拉取。GitHub 同时记录了[公开容器包的匿名拉取](ht
tps://docs.github.com/en/packages/learn-github-p
ackages/configuring-a-packages-access-control-an
d-visibility)与推荐的 [`GITHUB_TOKEN` 发布流程](https://
docs.github.com/en/actions/tutorials/publish-pac
kages/publish-docker-images)。

创建一个名为 `production` 的 GitHub Environment，
并配置这些 secret：

| Secret | 值 |
|---|---|
| `DEPLOY_HOST` | 安装的 SSH 主机名或 IP |
| `DEPLOY_USER` | `clumsies-deploy` |
| `DEPLOY_SSH_KEY` | 仅供 Actions 使用的专用 Ed25519 私钥 |
| `DEPLOY_KNOWN_HOSTS` | `DEPLOY_HOST` 固定的 SSH 主机密钥行 |

引导阶段把仓库变量 `SERVER_AUTO_DEPLOY_ENABLED` 设为 `false
`。等镜像包公开、受限部署身份验证通过后，再设为 `true`；
此后 `main` 上的绿色提交会自动部署。

不要上传个人或 root 的 SSH 密钥。生成专用密钥，把公钥复制到主机，
安装 Compose v2，然后从可信的发布检出运行安装脚本：

```bash
sudo apt-get install --yes docker-compose-v2
sudo deploy/server/install.sh /path/to/github-deploy-key.pub
```

安装脚本会创建 `clumsies-deploy`。
它在 `authorized_keys` 中的条目禁用 PTY、转发与用户 rc 文件，
并强制使用 `clumsies-github-command`。该命令只接受：

```text
deploy ghcr.io/lilhammerfun/clumsies-server@sha256:<64 hex> <40 hex commit>
```

该账号只能通过 `sudo` 调用经过校验的发布命令，无法获得可交互的部署 shell。

## 发布事务

`clumsies-server-release deploy` 在独占的主机锁下执行以下操作：

1. 校验 Compose v2 或更新版本、digest、commit、当前配置与公开来源；
2. 拉取不可变镜像并渲染 Compose 配置；
3. 创建并校验一份在线 PostgreSQL 备份；
4. 把该备份恢复到隔离的 PostgreSQL，用它启动目标镜像，
   运行真实的 SQLx migration，并要求 Server 健康；
5. 停止当前 Server，使切换期间不会发生写入；
6. 创建第二份无写入的 PostgreSQL 备份，并用 `pg_restore` 校验；
7. 原子地写入目标镜像 digest，只启动 Server，
   并要求容器健康与公网 HTTPS 健康同时成立；
8. 记录 commit、目标与先前镜像、两份备份、时间戳与结果。

健康探测通过本机边缘完成：把公开来源解析到 loopback。
因此过期的解析器缓存或正在进行的 DNS 迁移都不会让一次健康的发布被回滚。
当公网 DNS 还没指向本机时，发布会照常成功并记录一条警告。

如果切换后目标容器或公网健康失败，脚本会停止目标 Server，用那份无写入备份替换生产数据库，
然后启动并校验先前的镜像。数据库回滚与应用回滚是一次操作。
已发布的 migration 因此不需要对上一个 Server 镜像保持可读；
破坏性 migration 仍然需要 migration 测试，但不需要兼容实现。

要重试一次交付，用它的已发布 digest 与原始 commit 触发 `Server Deli
very`。破坏性 migration 之后，不要把旧镜像当作独立的回滚手段：
恢复这样的发布需要它记录的部署前数据库备份与先前镜像，作为一次恢复操作。
生产环境从不重新构建源码。

修改规范来源是一次独立的配置事务：

```bash
sudo clumsies-server-release reconfigure \
  https://memory.example.com \
  http://127.0.0.1/callback
```

该命令备份 PostgreSQL 与生效的环境文件，校验渲染后的 Compose 配置，
重建 Server 与 Caddy，等待容器健康与公网 HTTPS 健康；如果新的来源失败，
就恢复先前的环境与服务。

## 备份与恢复

安装脚本会启用：

- `clumsies-backup.timer`：每日自定义格式备份、校验、checksum，
  以及 14 天的本地计划备份保留；
- `clumsies-restore-drill.timer`：
  每周把备份恢复进隔离的 PostgreSQL 与 Server 栈，
  随后做真实的 Server 健康检查并自动清理。

也可以显式运行其中任一项：

```bash
sudo clumsies-server-release backup manual
sudo clumsies-server-release restore-drill
```

备份与发布记录位于 `/opt/clumsies/backups` 与 `/opt/clumsi
es/releases`。本地保留不是灾难恢复。请配置与安装组织相称的加密异地存储，
并从那份副本验证恢复；不要把数据库 dump 放进源码仓库或普通的 GitHub Actions
artifact。

## 可观测性

`deploy/observability` 提供一套可选、
面向单个安装的 Prometheus 栈：Prometheus、Alertmanager、
Grafana、node-exporter、cadvisor、
postgres-exporter，以及面向公网端点的 blackbox 探测。
它报告主机与容器资源、来自 Caddy 的 Server 请求速率与延迟、
PostgreSQL 连接数与库大小、备份与恢复演练的新鲜度、以及证书到期时间。
所有端口都绑定 loopback，告警通过 Alertmanager 用 SMTP 投递。

生产环境的 `deploy/Caddyfile` 打开一个全局块，
让 Caddy 在 Compose 网络的 admin 端点上暴露 HTTP 指标，
并对 Server 自己的 `/metrics` 路由返回 404，使采集端点留在内网。
安装步骤、必需的 `.env`、告警通道配置与告警规则都写在 [`deploy/observab
ility/README.md`](https://github.com/lilhammerfu
n/clumsies/blob/main/deploy/observability/README
.md)。

## 服务目标

下面这些成立时，安装就是健康的。数值刻意保守：没有冗余的单台主机不可能诚实承诺更多个 9。

| 目标 | 指标 | 依据 |
|---|---|---|
| 公网可用性 | 30 天内外部探测成功率 99.5% | blackbox exporter 的 `probe_success` |
| 写入新鲜度 | 99% 的客户端改动在五分钟内到达 Server | `clumsies_commits_total` 与边缘日志中的客户端活动对比 |
| 持久性 | 恢复点不超过 26 小时，恢复时间不超过 1 小时 | 备份新鲜度指标与每周恢复演练 |
| 请求延迟 | app 来源上的 p95 低于 1 秒 | `caddy_http_request_duration_seconds` |
| 服务端错误 | 少于 1% 的请求返回服务端错误 | 按状态码统计的 `caddy_http_request_duration_seconds_count` |

告警阈值跟随这些目标，而不是本地判断：备份规则在 26 小时触发，错误率规则在 1% 触发。
排队类信号（未完成 Draft、Reconciliation 冲突、未读通知）
在有对应目标之前只是积压报告。

架构变化时重新审视这些目标。高于 99.5% 的可用性需要单台主机提供不了的冗余，
部署文档必须先把这一点说清楚，再让人去承诺。

## 文档站点

文档站（`docs.clumsies.ai`）与官网（`clumsies.ai`）
是由同一个 Caddy 实例分别从 `/srv/docs` 与 `/srv/www` 提供的静态
站点（见 `deploy/Caddyfile`）。它们都在本仓库里：
VitePress 源码在 `docs/` 下，
官网在 `site/` 下（纯 HTML/CSS，无需构建）。

### 通过 CI/CD 部署

`CI` 在选中的检查于 `main` 上通过后调用 `Site Delivery`（`.git
hub/workflows/site-delivery.yml`）。`docs/`、
`site/`、Bun 依赖、`deploy/site.sh`、
Caddy/Compose 配置与 CI 交付策略的改动都会选中它。
只有 README 与截图改动不会部署站点。该工作流构建并部署经过测试的那个精确提交，
串行执行站点交付，并跳过已被更新站点改动取代的提交。Actions 页面仍保留手动触发，
用于显式重试或回滚。

除了 Server Delivery 的 secret，该工作流还需要这些仓库 secret：

| Secret | 值 |
|---|---|
| `SITE_DEPLOY_HOST` | 安装的 SSH 主机名或 IP（与 `DEPLOY_HOST` 同一主机） |
| `SITE_DEPLOY_SSH_KEY` | 专用私钥，其公钥在主机上被授权用于站点部署 |
| `SITE_DEPLOY_USER` | 可选；默认 `root` |
| `DEPLOY_KNOWN_HOSTS` | 主机固定的 SSH 主机密钥行（与 Server Delivery 共用） |

把 `SITE_DEPLOY_SSH_KEY` 的公钥加入站点部署用户的 `authorized
_keys`（若用 `root`，即 `/root/.ssh/authorized_keys`）
。与 Server Delivery 的密钥不同，这把密钥**不**受强制命令限制：
`deploy/site.sh` 需要一个能执行交互命令的 SSH 会话，
在主机上运行 `mkdir`、`rsync`、`scp` 与 `docker compose`。
两把密钥要保持独立，避免 Server Delivery 的强制命令被意外放宽。

### 手动兜底

不等待 CI 的一次性部署，可以在本地运行同一个脚本：

```bash
deploy/site.sh          # ssh 目标默认为 aliyun
deploy/site.sh my-host  # 也可以显式传入 ssh 目标
```

脚本会构建 VitePress 站点，用 `rsync` 同步两个静态根目录，
在目标上更新 `deploy/Caddyfile` 与 `compose.production.
yml`，并 reload Caddy 使同步过来的配置生效。
`docs.clumsies.ai` 与 `clumsies.ai` 的 DNS 必须指向该服务
器；Caddy 会自动申请 TLS 证书。

## 运维

```bash
sudo clumsies-server-release preflight
docker compose --project-name clumsies -f compose.production.yml ps
docker compose --project-name clumsies -f compose.production.yml logs server
systemctl list-timers clumsies-*
```

不要对已安装的组织使用 `docker compose down --volumes`，
它会删除 PostgreSQL 卷。
