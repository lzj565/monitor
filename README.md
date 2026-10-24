# monitor

文档：[monitor-document.pages.dev](https://monitor-document.pages.dev)，安装、配置、反向代理与主题开发都在这里。

主题站：[monitor-themes.pages.dev](https://monitor-themes.pages.dev)，在线预览各个公开页主题，复制地址即可在面板安装。

## 安装

先在 Linux 服务器上切换到 root 用户，再运行以下命令安装 hub：

```sh
su -
curl -fsSL https://github.com/lzj565/monitor/releases/latest/download/install-hub.sh | sh -s -- --yes
```

安装器会使用默认端口 `28080`。如需自定义端口或查看其他配置选项，请先下载脚本再运行 `sh install-hub.sh --help`。

## 特性

- 实时监控：秒级实时数据展示
- 轻量高效：Rust 语言构建，低资源占用，极简高效
- 自托管：完全掌控数据隐私，部署简单
- 通知：节点掉线、流量、到期与登录，推送到 Telegram 或自定义 Webhook

## 组成

| 仓库 | 说明 |
|---|---|
| [monitor](https://github.com/lzj565/monitor) | hub：后台、API、公开页宿主 |
| [agent](https://github.com/lzj565/agent) | Linux agent |
| [monitor-theme-default](https://github.com/monitor-probe/monitor-theme-default) | 内置默认主题 |
| [themes](https://github.com/monitor-probe/themes) | 主题站：收录第三方主题，提供在线预览 |

```
agent (Linux)  ──WebSocket / JSON-RPC 2.0──▶  hub (axum + SQLite)  ──▶  后台 + 状态页
```

## 配置目录

程序保留在 `/opt/monitor/`，配置集中在 `/opt/monitor/config/`：

| 文件 | 用途 |
|---|---|
| `agent.env` | hub 地址、Token、网卡和 sing-box API 端口 |
| `sb.json` | sing-box 配置 |

hub 数据库和主题仍在 `/opt/monitor/data/`；hub 启动参数仍在 systemd 服务中。后续接入 mieru 时，其配置也放入 `config/`。

安装器会在新配置不存在时复制旧配置：`agent.env` 从 `/opt/monitor/agent.env` 迁移；`sb.json` 优先从 `/opt/monitor/sb.json`，其次从 `/etc/sing-box/config.json` 迁移。原文件保留，新位置已有的配置优先。迁移及新建配置使用 `0600` 权限。服务名保持不变，仍接管现有 sing-box 服务。

agent 卸载会删除新旧位置的 `agent.env`，保留 sing-box 配置。升级需配套使用更新后的 agent 二进制与安装器。

## 开发与验证

Mac 使用 Docker / OrbStack 重新编译、启动和重启 Hub，见 [本地启动命令](LOCAL_RUN.md)。

后端按 API 业务、数据库查询及迁移分别放在 `src/api/`、`src/db/`；管理页面位于 `web-admin/src/components/admin/`，按路由加载。同步 HTTP 工作通过 `src/blocking.rs` 限制并发，并在阻塞线程执行。

```sh
npm --prefix web-admin ci
npm --prefix web-admin test
npm --prefix web-admin run lint
npm --prefix web-admin run build
sh scripts/theme.sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
sh scripts/check-agent-unit.sh
sh scripts/check-api-port-selection.sh
```

CI 检查 `main`、`dev` 和拉取请求。E2E 使用 `agent-test.pin` 固定的 Agent 版本及 SHA-256，下载来源与 `src/main.rs` 中的 `AGENT_REPO` 一致。更新兼容性测试版本时需同时更新 pin 中的版本和摘要。

备份恢复和主题上传的每个分片必须携带 `upload_id`（32 位十六进制随机值）、`offset` 和 `total`。同一文件的 ID、登录会话及总大小必须保持一致；每片最多 8 MiB，面板使用 4 MiB。不同上传使用独立临时文件，同一上传的写入、校验和发布互斥；恢复操作串行执行。未完成的上传保留一小时，过期后清理，重启后需重新上传。旧上传客户端需增加 `upload_id` 参数。

### 可选安装 Realm

单节点安装和批量添加中的“安装 Realm”开关默认关闭。开启生成 `--realm`，安装固定版本 2.9.6；关闭生成 `--no-realm`，执行后卸载本安装脚本管理的 Realm 程序和服务，保留配置。页面开关表示本次安装选择，不表示节点当前状态。

Realm 程序位于 `/opt/monitor/realm`，配置位于 `/opt/monitor/config/realm.json`，服务名为 `monitor-realm`。首次安装创建空转发规则，不启动、不设置开机启动。编辑配置添加 `endpoints` 后，systemd 使用 `systemctl enable --now monitor-realm`；OpenRC 使用 `rc-update add monitor-realm default` 和 `rc-service monitor-realm start`。配置说明见 [Realm 官方文档](https://github.com/zhboner/realm#configuration)。

重复安装保留配置及开机启动状态，正在运行的 Realm 在程序替换后重启。安装包由 Hub 通过已有 GitHub 代理中转，安装脚本校验固定 SHA-256，支持 x86_64、aarch64 Linux。未传 Realm 参数的旧安装命令和 `--upgrade` 不改变 Realm；升级时也可显式附加 `--realm` 或 `--no-realm`。`--uninstall` 仍仅卸载 Agent。第三方 Realm 安装不属于此开关管理范围。


### Realm 页面管理

在管理菜单「代理」中点击「新增代理」，类型选择「Realm 转发」。入口直接选择「节点」里已有的机器，填写对外域名或 IP、监听端口，再选择目标节点及其已有代理；无需单独添加 Realm 机器。首次使用入口节点时，在同一表单确认面板接管本安装器的 Realm 配置。节点需要支持 `realm.v1` 的 Agent；未安装时提供带 `--realm` 的安装命令，旧 Agent 需同时升级。

普通代理和 Realm 规则显示在统一列表，可按类型、入口节点和状态筛选。Realm 行可编辑、启停、删除并查看流量与入口服务；「节点 Realm 服务」可管理已有节点的共享服务设置。对外地址、服务启停和重启影响同一节点全部 Realm 规则，修改共享配置需明确确认。旧 `/admin/realm` 链接自动跳转到代理页的 Realm 筛选。当前支持 TCP，监听地址默认 `0.0.0.0`，可选 `::`。保存后 Hub 自动生成完整配置并通过 Agent 应用；有有效规则时自动启动并设置开机启动，停用机器或最后一条有效规则后停止并取消开机启动。首次接管会保留 `realm-panel-original.json`，应用失败恢复旧配置及服务状态。

应用确认成功后，用户中心及 Clash / sing-box 订阅新增 Realm 入口，原直连入口保留。中转复用目标代理的认证、授权和额度；禁用规则、撤销授权或用户过期立即停止发布对应入口。目标地址变化时自动重新同步，过期结果不能发布新版本。对外地址必须从用户侧可达；端口防火墙不由面板自动修改。

机器状态每 5 秒刷新。Agent 目标 TCP 检测每次最多检查 8 个目标，页面显示检测数量；进程运行和 TCP 可连接均不代表 VLESS 认证已通过。面板只接管安装器管理的 `monitor-realm` 服务，不接管第三方 Realm，也不支持手工配置与面板配置混用。

数据库迁移到版本 22，Realm 机器和规则纳入备份。部署顺序为先升级 Agent，再部署 Hub。两仓库的 Rust 测试覆盖配置回滚、过期修订、授权和备份兼容；`scripts/e2e-realm.py` 是供隔离 Linux Docker 使用的真实 Realm + Agent + Hub 联调脚本，依赖分别挂载到 `/hub`、`/agent` 的 debug 二进制和 `/realm.tar.gz` 的固定 Realm 包。脚本拒绝覆盖已有 `/opt/monitor/config`；systemd 服务调用在测试容器中由适配器模拟，真实转发由 Realm v2.9.6 执行。
