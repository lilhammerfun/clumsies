# 认证与会话

## 登录流程

Clumsies 使用组织的 OpenID Connect 提供方。
Desktop 在系统浏览器中发起流程，并在一个临时的 `127.0.0.1` 端口上监听最终的
授权码。
客户端与提供方的两次交换都使用带 `S256` 的 PKCE；
在 Server 返回短时效的客户端授权码之前，会先校验 state 与 OIDC nonce。

在开始初始化或登录之前，原生 App 会校验并持久化一个规范化后的 Server 来源。
远程来源必须使用 HTTPS；只有 loopback 开发场景才接受 HTTP。凭据、路径、
查询串与片段都会被拒绝，使认证与 API 请求不会漂移到另一个权威。

```text
Desktop native
  -> Server /oauth2/authorization/oidc
  -> organization OIDC provider
  -> Server /login/oauth2/code/oidc
  -> Desktop loopback callback
  -> Server /api/v1/auth/token
  -> daemon credential store
```

SwiftUI 视图永远拿不到 access token 或 refresh token。
所有需要认证的产品请求都经由 daemon 发出，
由它在 App 的呈现状态之外注入 bearer token。

## 首次安装

当 `GET /api/v1/setup` 报出 `setup_required` 时，
App 显示原生初始化界面，而不是打开 Server 托管的页面。
它用部署的 Setup Code 换取一个 HttpOnly 的 setup cookie，
把返回的 CSRF token 只保存在原生内存中，并提交组织名称、
默认 Project 与可选的邮箱域名允许列表。

随后 App 启动常规的 loopback 授权流程，并把同一个 `redirect_uri`、
state 与 `S256` challenge 发送到 `POST /api/v1/setup
/oidc-authorizations`。提供方校验通过后，
Server 在一个事务中创建组织、首位 Owner、默认 Project、
身份绑定与初始 Ref。它把客户端授权码返回给 loopback 回调；
App 用自己的 verifier 兑换该授权码，并把 token 对安装进 daemon。
初始化永远不会创建浏览器管理员会话，已完成的安装也不能被再次认领。

## 提供方校验

Server 从配置的 issuer 发现提供方元数据与 JWKS，
并要求发现文档返回完全相同的 issuer。授权码交换失败使用 `oidc_code_excha
nge_failed`；issuer、audience、nonce、
过期与签名失败使用 `oidc_id_token_invalid`。

当 ID Token 引用了未知的签名密钥时，Server 会刷新一次发现文档与 JWKS，
然后重新校验已经收到的 token。它不会第二次兑换一次性的提供方授权码。
对匹配密钥的签名失败会直接拒绝，不做刷新重试。

## 成员准入

组织 owner 或 admin 通过 admin API 创建成员准入记录。
首次成功 OIDC 登录时，Server 用校验过的邮箱匹配该记录，
并把稳定的 `(issuer, subject)` 身份绑定上去。之后的登录直接解析外部身份，
因此邮箱声明变化不会把身份迁移到另一个用户。未知、已禁用、未验证或域名不被允许的身份都会被拒绝。

只有原生首次安装会创建初始 Owner。此后每个成员都必须由既有的组织 owner 或管理员准入
。

## Token 生命周期

Server 签发不透明的 access 与 refresh token，
PostgreSQL 里只保存哈希。refresh 是轮换的：
被出示的 refresh token 会被吊销，并返回新的 access/refresh 对。

daemon 遇到 `401 Unauthorized` 时会尝试一次 refresh 和一次请
求重试。它不会通过健康检查、Project 配置、IPC 响应或渲染层状态泄露任何机密。

App 内的 Administration 界面通过 daemon 使用同一套 bearer 凭
据。如果 daemon 无法启动，**Administrator Recovery** 会直接面
向受信任的 Server 来源执行原生 OIDC 流程，并只在 App 内存中保留短时效凭据；
它可以在不创建 cookie 会话的前提下检查健康、修复成员访问与吊销 token。

登出会调用 `DELETE /api/v1/auth/session`，
吊销当前会话并清除本地凭据。

## Server 必需配置

| 变量 | 含义 |
| --- | --- |
| `CLUMSIES_PUBLIC_ORIGIN` | 规范的 HTTPS Server 来源；本地开发允许 loopback HTTP |
| `CLUMSIES_OIDC_ISSUER` | 组织 OIDC issuer，必须完全一致 |
| `CLUMSIES_OIDC_CLIENT_ID` | OIDC 机密客户端 ID |
| `CLUMSIES_OIDC_CLIENT_SECRET` | OIDC 机密客户端密钥 |
| `CLUMSIES_CLIENT_REDIRECT_URIS` | 逗号分隔的额外受信任客户端回调 |

Server 从 `CLUMSIES_PUBLIC_ORIGIN` 推导提供方回调 `/logi
n/oauth2/code/oidc`。原生客户端回调单独列入允许列表，
因此 Server 永远不会把授权码重定向到不受信任的来源。

对 Desktop 的动态 loopback 端口，
配置 `CLUMSIES_CLIENT_REDIRECT_URIS=http://127.0.0
.1/callback`。缺失端口是刻意的模板：
Server 只对确切的 loopback 主机、协议、路径与查询串接受任意临时端口。

Desktop 使用的远程 Server URL 必须是 HTTPS。
纯 HTTP 只在 loopback 开发地址上被接受。

## 本地凭据存储

daemon 在 macOS 钥匙串中保存一个 generic-password 条目。
service 为 `ai.clumsies`，
account 为 `server-session`，
加密值包含 Server URL 与 access/refresh token 对。
Server URL 把凭据绑定到某一个端点；当钥匙串会话与配置的 Server 不匹配时，
daemon 会拒绝加载。

SQLite 只保存非机密的 Server 与 Project 配置。
登录与 token 刷新会作为一条记录替换钥匙串的值，
而清除 daemon 会话或无效的 refresh 会话则删除它。
没有 SQLite 或明文文件形式的凭据兜底。测试注入隔离的凭据存储；
另有一条单独开关的冒烟测试会走真实的 macOS 钥匙串。
