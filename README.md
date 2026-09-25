# monitor

文档：[monitor-document.pages.dev](https://monitor-document.pages.dev)，安装、配置、反向代理与主题开发都在这里。

## 安装

在支持的 Linux 服务器上运行以下命令，安装器会从 GitHub 下载最新发布版并配置 systemd 服务：

```sh
curl -fsSL https://github.com/monitor-probe/monitor/releases/latest/download/install-hub.sh | sudo sh
```

默认监听端口为 `28080`。交互式终端中可按提示设置端口和站点地址；非交互执行会使用默认配置。更多配置方式见[安装文档](https://monitor-document.pages.dev)。

## 特性

- 实时监控：秒级实时数据展示
- 轻量高效：Rust 语言构建，低资源占用，极简高效
- 自托管：完全掌控数据隐私，部署简单
- 通知：节点掉线、流量、到期与登录，推送到 Telegram 或自定义 Webhook

## 组成

| 仓库 | 说明 |
|---|---|
| [monitor](https://github.com/monitor-probe/monitor) | hub：后台、API、公开页宿主 |
| [agent](https://github.com/monitor-probe/agent) | Linux agent |
| [monitor-theme-default](https://github.com/monitor-probe/monitor-theme-default) | 内置默认主题 |

```
agent (Linux)  ──WebSocket / JSON-RPC 2.0──▶  hub (axum + SQLite)  ──▶  后台 + 状态页
```
