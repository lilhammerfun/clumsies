# 本地 HTTP 业务负载测试操作说明

此工具用于在同一台开发机上复现指定请求速率下的 HTTP 业务表现。它创建自己的 PostgreSQL 容器、64 个测试项目和一个 Owner，启动由生产应用构造函数生成的真实 TCP Server，再运行 Python 标准库客户端。它不会连接已安装的 App、daemon、钥匙串或现有数据库，也不接受 Server URL。

## 准备与运行

需要仓库支持的 Rust/Cargo、Python 3 和本机 Docker。Docker 端点必须是本地 Unix socket；PostgreSQL 和 HTTP 只绑定 IPv4 loopback。数据库使用 `postgres:16-alpine`，数据保存在这次容器的临时匿名卷中。

先检查调度、分位数和边界处理；此命令不启动业务负载：

```sh
python3 dev/load-http.py --self-check
```

在其他编译和测试结束后，分别运行两类负载。每个命令都重新创建测试数据，输出路径必须尚不存在：

```sh
python3 dev/load-http.py --profile read --rate 10 --duration 20 --workers 16 --deadline 5 --output /private/tmp/clumsies-load-read.json
python3 dev/load-http.py --profile read-write --rate 10 --duration 20 --workers 16 --deadline 5 --output /private/tmp/clumsies-load-read-write.json
```

工具先通过 `cargo test --release --locked -p server --test observability_load --no-run` 构建 fixture，然后启动数据库和 HTTP 服务。编译、迁移、建数和两次 `/metrics` 快照请求均在统计窗口之外。`observability_load` 默认被忽略，普通 `cargo test` 不会产生业务负载。

参数含义：

| 参数 | 含义 |
|---|---|
| `--profile` | `read` 或 `read-write`，必填 |
| `--rate` | 每秒计划到达的业务操作数，默认 10 |
| `--duration` | 安排到达事件的秒数，默认 20；到达结束后等待已安排操作的终态 |
| `--workers` | 客户端工作线程数，默认 16，范围 1–64 |
| `--deadline` | 从计划到达开始计算的总预算秒数，默认 5，包含客户端排队 |
| `--output` | 新建的 JSON 结果文件，必填；不会覆盖旧结果 |

每次最多安排 100,000 个操作。速率、时长和预算必须是有限正数。需要比较多个速率时，逐次运行上述命令，并为每次运行指定不同输出文件；不要同时运行多个实例。

## 固定数据与请求组合

初始数据为 64 个项目、一个 Owner，每个项目一个成员和 256 字节描述。HTTP Server 使用 4 个 Tokio worker，数据库连接池上限 10、获取连接超时 5 秒。数据库连接、迁移和业务 handler 均使用现有实现；认证初始化复用测试 OIDC provider，测量期间的请求通过真实 Bearer 验证。

`read` 循环执行项目列表（第一页 50 条）、项目详情、项目成员列表三个操作。

`read-write` 每四个计划到达中，有三个上述读操作和一个项目描述更新。更新操作先 GET 当前 revision，再带 `If-Match` PATCH 256 字节描述，并检查 revision 确实增加；两个 HTTP 请求算一个业务操作。写入在 64 个项目间轮转，描述值交替，项目数量保持不变。同一项目的写操作在客户端互斥，等待计入客户端排队。每个客户端线程复用自己的 HTTP 连接。

这是项目元数据负载，不包含查询召回、Draft/Review 发布、blob 传输或 daemon 同步。不同业务负载需先另行定义数据与请求组合，再比较结果。

## 读取结果

`summary.planned` 是全部计划到达数；`actual` 是实际开始 HTTP 业务操作的数量；`completed` 是已经得到终态的计划数，包含排队超时。`success` 只包含在预算内完成且响应内容通过校验的操作。`http_attempts` 与 `http_responses` 单独记录网络请求数，避免将读写组合操作当成单个 HTTP 请求。

工具按照固定时刻安排到达，不等待前一个请求结束再安排下一个。客户端忙碌、调度延迟或预算已经耗尽时，事件仍被记录；尚未发出的事件标为 `queue_deadline`，不会从分母中消失。已经发出但超时的操作标为 `deadline`，HTTP、传输和响应内容错误单独分类。不会自动重试失败的业务请求。

`successful_latency_ms` 按“完成时刻减计划到达时刻”统计成功操作 p50/p95/p99，包含客户端排队；`all_planned_latency_ms` 包含失败与未发出的计划。`client_queue_ms` 是计划到达至首次 HTTP 开始的时间；`transport_ms` 是实际开始至终态的时间，读写操作包含两次 HTTP 往返。原始 `samples` 保留每个事件的计划、提交、实际开始和终态时刻，可检查调度是否跟上计划。无样本分位数为 `null`；分位数采用 nearest-rank，样本很少时应结合样本量阅读。

`offered_operations_per_second` 是计划数量除安排时长；`success_operations_per_second_including_drain` 用全部成功操作除从首次计划到最后终态的实际时长。二者统计边界不同，不能互换。`actual_in_scheduling_window` 和 `drain_seconds` 用于识别客户端追不上安排的情况。报告还保存两次 Server 指标快照、源码 revision/是否有本地改动、实际可执行文件 SHA256、源码整体 SHA256 与逐文件 SHA256、宿主和 PostgreSQL 版本、数据库镜像 ID，以及本地 Docker engine 的 CPU/内存配置。

源码指纹覆盖 Cargo manifests/lock、Server src/migrations、测试 common、负载 fixture/driver，以及存在时的 Rust toolchain/Cargo 配置；报告中的逐文件列表明确记录实际范围。整体指纹按排序后的相对路径、NUL、二进制文件 SHA256 和换行逐项计算。编译前后指纹必须一致，保证本地改动也能核对。

固定到达计划和完整事件分母避免了仅统计成功发出的请求所造成的 coordinated omission。Python/GIL、工作线程数、客户端项目锁和同机资源竞争仍会影响结果；`transport_ms` 是客户端观察，不能当作纯 Server handler 耗时。响应的 socket 超时与最终预算检查会将超过计划预算的返回归为失败；这不意味着服务端事务已经取消，描述写入可能在客户端超时后完成。

## 退出与清理

工具为这次运行标记唯一容器身份，只删除自己创建的容器和匿名卷。HTTP 服务和数据库连接池正常退出后才接受 fixture 成功。任何未成功操作、异常、强制终止或清理失败都会令命令非零退出；结果中的 `status` 和 `fixture_cleanup` 同时标记失败，不会因为已经写出延迟统计而声称成功。

Ctrl-C 或 SIGTERM 会触发清理。SIGKILL 和宿主断电无法执行进程内清理；若发生，应仅查看 `clumsies-load-` 名称及 `clumsies.load.fixture` label，确认这次运行身份后删除对应测试容器。不要删除其他实例或持久卷。

首次结果的范围是本机生成器、本机 Rust 应用和本机 Docker 中的 PostgreSQL。报告不包含 Caddy/TLS、正式日志写入、公网或 VPS 磁盘/CPU/网络条件，不能据此推算生产 VPS 容量或承诺 SLO。
