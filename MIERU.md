# mieru 接入

首版为每节点最多一条 mieru 代理、一个 TCP 端口（1025–65535）。可以与 VLESS 共存，支持经 Realm TCP 转发。服务端程序为 mita，固定版本 3.39.0。

## 安装与使用

先升级 Hub，再升级 Agent。面板的节点安装命令提供 mieru 和 sing-box 开关。命令行示例：

```sh
# 安装 Agent、必装的 sing-box，以及 mieru（当次覆盖）
sh install.sh --server https://hub.example.com --token TOKEN --mieru
# 同时安装 sing-box、mieru 和 Realm
sh install.sh --server https://hub.example.com --token TOKEN --sing-box --mieru --realm
# 升级读取 Hub 已保存的安装选择
sh install.sh --upgrade
# sing-box 每次安装及升级均为必装，保留兼容参数
sh install.sh --upgrade --sing-box
# 移除托管的 mita 程序及服务，配置和统计文件保留
sh install.sh --upgrade --no-mieru
```

sing-box 固定必装，`--skip-sing-box` 已拒绝。省略组件参数时读取 Hub 保存的安装选择，关闭 Realm/mieru 会在执行时卸载托管组件。`--no-mieru` 可覆盖当次选择并停止服务。mieru 卸载后保留所有权标记，允许后续重新安装，拒绝接管系统中非本安装器管理的 mita。

在代理列表新增 mieru，选择节点、对外地址和端口，然后给用户授权。没有有效授权用户时服务保持停止。所有用户共用一个自动生成的 mieru 密码，与面板登录密码独立；连接用户名仍为各自的 `user-{id}`。管理员在代理管理页的“mieru 连接密码”入口统一查看、复制和重置；用户列表不再提供配置入口。

Realm 规则直接选择 mieru 作为目标代理。用户门户提供 Mihomo YAML 配置和 `mierus://` 分享链接；Clash / Mihomo 订阅包含 VLESS、mieru 直连及 Realm 入口。sing-box 订阅只输出 VLESS。mieru 使用 TCP、UDP 转发开启、MULTIPLEXING_OFF 和 HANDSHAKE_NO_WAIT。

## 接口和存储

- `proxies.protocol=mieru`，配置为 `{"transport":"tcp"}`；复用用户、授权及排序表。
- 新增 `user_proxy_credentials`、`mieru_hosts`、`mieru_traffic_sources`，数据库版本为 26；纳入备份检查和恢复。
- `GET /api/mieru/hosts` 返回能力、在线、期望版本、已应用版本及错误。
- `GET /api/mieru/hosts/{id}/config` 为管理员配置预览。
- `GET/POST /api/mieru/credential` 为管理员读取/重置连接凭据。
- `GET /api/subscriptions/{token}/clash` 统一导出 VLESS / mieru；沿用用户订阅令牌、停用和到期检查。独立 `/mieru` 订阅格式已移除，旧链接返回 404，需重新导入 Clash / Mihomo 链接。
- `/mieru/x86_64`、`/mieru/aarch64` 中转官方发布文件；安装器使用固定 SHA256。
- Agent 能力为 `mieru.v1`、`mieru.traffic.v1`；管理命令为 `mieru.status/config.get/config.check/config.apply/restart`。

托管程序 `/opt/monitor/mita`，配置 `/opt/monitor/config/mieru.json`，状态 `/opt/monitor/config/mieru-state.json`，服务 `monitor-mieru`。配置权限 0600，服务以 mita 用户运行，管理 socket 位于 `/run/monitor-mieru/mita.sock`。上游持久化指标位于 `/var/lib/mita/metrics.pb`。

mita 没有独立的只读配置校验命令，`replace config` 会通过 RPC 修改守护进程配置。Agent 对首版支持的配置子集做严格离线校验，再原子替换完整 JSON；用户更新使用 reload，监听变化重启服务，失败恢复旧配置及运行状态。版本和摘要拒绝过期命令，Hub 持续检查并重试；恢复旧 Hub 数据时根据 Agent 状态提高版本后重新下发。

## 流量口径

每 5 秒读取一次 mita JSON 指标，超时 3 秒。用户名使用稳定的 `user-{id}`，不跟随登录名变化；所有用户共用一个全局随机密码，在代理管理页统一查看和重置。旧的个人密码不再使用，升级后需更新订阅。mieru 原始计数基线与 sing-box 分离，增量事务写入现有用户、节点流量汇总。面板清零不清空 mita 指标或原始基线。

用户用量包含 VLESS 和 mieru。经 Realm 的连接只在目标 mieru 计用户用量；Realm 入口转发量和节点网卡流量独立展示，不加进用户用量。首次采样、计数回退和守护进程代次变化重新建立基线；缺失计数不清零，失败报告不入账。

本功能用于监控。重启、采样间隔及上游指标落盘周期可能造成缺口，不承诺精确计费；不新增硬配额、设备限制、UDP 或端口范围支持。

## 验证

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cd web-admin && npm test && npm run lint && npm run build
sh scripts/check-mieru-install.sh
sh scripts/check-realm-install.sh
sh scripts/check-agent-unit.sh
sh scripts/check-api-port-selection.sh
```

真实 Linux 集成测试使用一次性容器：放置官方 ARM64 mita、mieru、Realm，创建 mita 用户及可写指标/socket 目录；仅在容器内将 `scripts/e2e-mieru-systemctl.sh` 放到 `/usr/bin/systemctl`，作为服务管理包装器。放置新 Agent 的测试二进制、`scripts/e2e-mieru.py`，创建 `/tmp/monitor-mieru-disposable` 后运行 `scripts/run-mieru-e2e.sh`。该测试覆盖原生客户端直连、经 Realm 转发、用户统计、幂等应用、过期命令拒绝、用户重载、端口冲突回滚和停服。

服务包装器不能安装到真实机器。容器中的测试数据和回环目标许可均仅用于测试；生产配置默认不允许代理到回环地址。真实 systemd/OpenRC 宿主机服务生命周期及 x86_64 真机运行仍需部署环境验收。


### mieru 首次接入验证（2026-10-24，组件记录改动前）

- Hub：236 项 Rust 测试通过，fmt 和严格 Clippy 通过。
- Agent：62 项常规 Rust 测试通过，fmt 和严格 Clippy 通过；另一个默认忽略的真实 mita 集成测试已在一次性容器中单独执行通过。
- 前端：测试、lint、TypeScript 和生产构建通过。
- 安装脚本：mieru、Realm、Agent unit、sing-box API 端口选择检查通过；mieru 安装检查已加入 CI，但此次未推送，未运行远程 CI。
- Linux ARM64 原生 mita/mieru 3.39.0 + Realm 2.9.6：直连及 Realm 转发各完成 23552 字节上下行往返，用户统计采集通过。
- 真实 Hub HTTP + 新 Agent WebSocket：授权、原生导出、密码轮换、撤销后的自动停服通过；混合节点删除最后一条 VLESS 后清理旧配置有单独回归用例。
- 容器以测试服务包装器模拟 init 管理，因此不等同于真实 systemd/OpenRC 宿主机验收；x86_64 真机运行和生产部署未测。

额外的 HTTP/WebSocket 验证脚本为 `scripts/e2e-mieru-management.py`，在同一个一次性容器中放置新 Hub/Agent 二进制后执行。`agent-test.pin` 保留已发布 Agent 的兼容性测试基线；发布含 mieru 的 Agent 后，再按实际发布资产更新该 pin，不能预先填写未发布资产的校验值。

组件安装选择持久化、迁移默认值及状态上报详见 [NODE_COMPONENTS.md](NODE_COMPONENTS.md)。
