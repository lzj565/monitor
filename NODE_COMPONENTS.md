# 节点组件

Hub 保存节点的 sing-box 安装选项及 Agent 上报的实际状态。新节点默认关闭 sing-box。在节点安装窗口保存选项后，执行生成的命令安装或卸载托管程序。关闭 sing-box 时保留其配置。

安装器使用节点 Bearer token 请求 `GET /api/agent/components`。Hub 返回 `singbox=0` 或 `singbox=1`。安装器仍可读取旧 Hub 的组件配置，并在升级时清理带 Monitor 所有权标记的 Realm 和 mieru 服务。

使用 `sh scripts/check-component-profile.sh` 验证配置解析。
