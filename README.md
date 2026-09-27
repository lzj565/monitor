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
