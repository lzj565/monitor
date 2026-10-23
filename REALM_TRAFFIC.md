# Realm 入口端口流量

## 行为与范围

- 官方 Realm 保持不变；Agent 独立采集，不控制转发启停，不修改用户配额或倍率。
- 仅客户端侧 TCP：input 监听目标端口为上传，output 监听源端口为下载；排除 lo。不统计到目标节点的另一段连接，不和整机网卡流量相加。
- 包字节包含协议头与重传，并非纯载荷或运营商账单。默认 IPv6 通配监听同时映射 IPv4/IPv6；系统/Realm 若限制 IPv6-only，实际只产生 IPv6 计数。
- 页面展示机器、规则累计上下行、今日、本月、速率、最近采集、缺口次数，保留已删除规则历史。日/月按 UTC；5 秒采样按采样结束分钟归档，边界误差最多一个采样间隔。
- 未开始采集显示“—”，已知历史总量保留；离线、过期或未监控时速率显示“—”。

## 可用性与防火墙边界

Agent 需要已有 nft 命令、内核 nftables 支持及 CAP_NET_ADMIN。仅操作 `inet monitor_realm_traffic`，随机标记链验证归属；同名第三方表会被拒绝。独立 input/output 基础链只做计数，默认 accept，无 drop、无全局 flush。TCP 跳转链明确隔离 UDP。

首次有有效规则时检测。缺 nft、权限不足、规则/JSON 不兼容、计数器消失或回退，均锁定为“未监控”。不安装 nft、不切换 iptables/eBPF、不影响 Realm。状态持久化，刷新、配置改变、WS 重连及 Agent 重启均不重新探测。只有页面“重新检测流量监控”的新随机请求才能重试。健康状态的系统重启可重建专属计数器，并记录缺口。

停用/删除规则会切换专属映射，结算退役计数器后删除计数对象。没有有效规则时停止轮询 nft。改端口保留逻辑流量身份；移动至另一台机器生成新身份，原机器历史保留。

## 数据链路

- 独立线程持有 SQLite 与 nft 子进程；子进程有 3 秒超时，输出最多 2 MiB，不在网络 runtime 阻塞。
- `/opt/monitor/config/realm-traffic.db`，私有权限，FULL/WAL；页数限制 56 MiB，为 WAL 留余量，缓存 512 KiB。在线清理已确认旧分钟，离线最多保留 7 天；接近容量时丢最旧分钟并标记缺口。
- 每 5 秒读取非重置计数器，保存分钟累计值与 revision。进程重启、长采样间隔、回退或重检间隙不补造数据；重新建立基线并记录缺口。
- `realm.traffic.v1` 能力协商。Hub 仅在 Realm 已确认应用且监听集合一致后配置统计。
- 每帧最多 128 条，Hub 上限 48 KiB；Agent 分批重放历史，ACK 精确匹配 collector / 行 ID / revision。行 ID 不复用，避免旧 ACK 确认新记录。
- Hub 唯一键 `(node_id, collector, traffic_key, minute)`，仅更新更高 revision 且不回退的累计值，重传不重复累加。持久化后才 ACK。校验节点归属、时间、数值和批量大小。备份恢复后可能缺少离线队列对应的规则身份，这类记录不猜测归属，确认丢弃并显示“无法归属记录”，避免阻断重连。
- Hub schema 24；无旧 Realm 数据迁移，仅重建空的旧配置表，运行状态保留，非空旧配置或流量历史拒绝升级。旧 Agent 仍可管理 Realm，页面提示升级后才能监控。需要同时更新 Hub、Agent；没有在本次工作中发布或部署。

## 验证

- Hub 228 项 / Agent 61 项 Rust 测试通过，格式与 Clippy 通过；前端 lint、6 组脚本测试、生产构建通过。
- 隔离 Docker、CAP_NET_ADMIN、nft 1.0.6、官方 Realm 2.9.6：第二容器产生 IPv4 TCP 数据，入口双向各 708 字节进入 Agent 账本并被 Hub ACK；loopback 不计入，UDP 探测不增加 TCP 计数。编辑监听端口后保持身份与累计数据；转发、订阅发布/撤回、停止和删除历史保留通过。
- 受控 nft 失败：普通刷新及 Agent 重启不新增探测，手动重检触发探测；期间真实 Realm TCP 转发继续通过。
- 确认去重、旧 ACK、计数器回退、断线分钟重放、跨日归属、删除历史、保留期限与行 ID 不复用。
- 未做生产部署、浏览器交互验收、真实非 loopback IPv6、连续 7 天离线或 1024 规则容量压测。

`scripts/e2e-realm.py` 仅允许全新 Docker 容器；需要挂载编译后 Hub 到 `/hub` 与 `/work`、Agent 到 `/agent`、官方 Realm 压缩包到 `/realm.tar.gz`，并安装 python3/systemd。普通模式验证转发；`REALM_TEST_NO_NFT=1` 验证降级锁定；`REALM_TEST_TRAFFIC=1` 需 CAP_NET_ADMIN/nft，等待 `/tmp/realm-traffic-port` 后，从第二个容器连接测试容器 IP 与该端口发送 echo 流量；测试修改端口后该文件会更新，需再次发送流量。systemctl 是测试适配器，并非完整 systemd 启动验收。测试禁止 host 网络，禁止挂载实际 `/opt/monitor`。
