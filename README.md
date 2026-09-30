# monitor

文档：[monitor-document.pages.dev](https://monitor-document.pages.dev)，安装、配置、反向代理与主题开发都在这里。

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
