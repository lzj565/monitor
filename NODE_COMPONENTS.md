# 节点组件安装记录

每个节点在 Hub 的同一个 SQLite 数据库中保存 `singbox`、`realm`、`mieru` 三条 `node_components` 记录。数据库版本为 26。

## 安装选择与实际状态

- `desired_installed`：保存的安装选择。sing-box 固定开启；新节点和旧数据迁移时 Realm、mieru 默认关闭。
- `installed`、`managed`、`running`、`version`：Agent 最近一次检查到的实际状态，与安装选择独立。
- `checked_at`：上次检查结果接收时间，初始为空；空值在页面显示“未上报”，不将默认 false 当作已实测。
- `last_error`：组件检查失败的原因。失败或旧 Agent 缺失字段不清空已有实际状态。

迁移不会根据代理表推测安装情况，不会安装、停止或卸载节点上的任何组件。已有 Realm/mieru 的老节点首次重装前，应先开启并保存对应选择，否则执行重装会按默认关闭选择卸载托管组件。关闭 Realm/mieru 后保存只更新数据库；之后执行安装或升级命令才会停止并卸载托管程序，配置和统计文件保留。

## 面板操作

节点的安装窗口自动读取已保存选择，sing-box 显示“必装”。修改 Realm/mieru 后点击“保存安装选项”；未保存、读取失败或保存失败时不显示可执行的安装命令。取消已安装组件时会提示执行后卸载的影响。

重新打开窗口、刷新页面或重启 Hub 均保留已保存选择。批量注册命令携带初始选择，节点注册时原子写入；在已注册机器上重跑注册命令不会用批量默认值覆盖已有选择，安装器也会忽略该批量命令的组件默认参数，读取该节点已保存选择。Realm 页面生成安装命令前也要求节点已保存开启 Realm。

## 接口与安装器

- `GET/PUT /api/nodes/{id}/components`：管理员读取或保存组件选择和实际状态。
- `GET /api/agent/components`：节点 Bearer token 认证，只能读取自身保存的安装选择。返回严格的三行格式 `singbox=1`、`realm=0|1`、`mieru=0|1`。
- Agent `components.v1` 能力通过 `components.status` 上报状态，连接后立即采集，以后每 30 秒采集；后台采集、超时及单结果队列不阻塞普通指标和命令。Hub 校验当前连接及能力，并限制上报频率。

```sh
# 默认按 Hub 已保存的选择安装，不需要重新选组件
sh install.sh --server https://hub.example.com --token TOKEN
# 升级也读取 Hub 已保存选择
sh install.sh --upgrade
# 显式参数只覆盖本次运行，不修改 Hub 保存选择
sh install.sh --upgrade --realm --no-mieru
```

安装器先完成注册或读取现有凭据，再获取安装选择；获取失败或响应格式错误时，在替换程序、操作服务前停止。首次注册成功后会先将凭据暂存到 root 专用的 `config/registration.env`（0600），失败后重试同一注册命令会复用原节点，完整安装写入正式环境文件后清除暂存。sing-box 始终安装；旧 `.skip-sing-box` 标记不再控制选择，安装成功后清除。`--skip-sing-box` 明确拒绝，`--sing-box` 保留兼容。之前只装 mieru 的节点升级时会补装 sing-box。

组件的实际状态检查针对 `/opt/monitor` 下的组件程序及对应 systemd/OpenRC 服务，不扫描未知名称的第三方服务。非托管文件继续受到现有安装器所有权检查保护。新表纳入数据库备份、恢复和数据管理；组件记录不出现在公开节点接口。

先升级 Hub，再升级 Agent 和使用新安装器。旧 Agent 不上报组件状态，但保存的安装选择和安装命令仍可使用；新安装器连接缺少新接口的旧 Hub 时会停止。

## 验证命令

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
sh scripts/check-component-profile.sh
sh scripts/check-realm-install.sh
sh scripts/check-mieru-install.sh
sh scripts/check-agent-unit.sh
sh scripts/check-api-port-selection.sh
cd web-admin && npm test && npm run lint && npm run build
```


## 本次验证（2026-10-25）

- Hub 243 项 Rust 测试通过；包含 v25 迁移、固定必装约束、注册选择持久化、缺失/错误上报保留旧值、旧连接隔离及安装器认证接口。
- Agent 64 项常规测试通过，原有真实 mita 测试默认忽略；本次改动通过 fmt 和严格 Clippy。
- 安装脚本测试及全量 ShellCheck 通过；覆盖四种组件组合、显式覆盖、错误/不完整响应、下载失败、禁止跳过 sing-box，以及失败注册凭据的私有暂存与跨 Hub 隔离。
- 前端测试、lint、TypeScript 和生产构建通过；Chrome 实测重新打开/刷新回填、保存与读取失败隐藏命令、卸载提示及 375px 布局。脚本：`scripts/e2e-components-ui.mjs`，使用模拟 API。
- 一次性 Linux ARM64 容器内，真实 Hub/Agent 和实际安装器解析函数联调通过；验证状态上报、保存选择不等于已安装、公开接口隔离及 Hub 重启后保留选择。组件程序及 init 状态使用受控测试夹具，不能视为真实宿主机安装验收。脚本：`scripts/e2e-components.py`。
- 未部署生产，未运行远程 CI；真实 systemd/OpenRC 宿主机的完整安装流程仍需部署环境验收。
