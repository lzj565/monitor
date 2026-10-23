# Realm 实现与验收

基于 Hub `911e63a` 与 Agent `f92d99d`，两个仓库分别实现。

## 已实现

- 当前入口为 `/admin/proxies`：普通代理和 Realm 统一管理，新增时选择 Realm，直接使用已有节点并配置监听端口到已有代理。旧 `/admin/realm` 链接重定向到 Realm 类型筛选。保存自动同步、离线待同步、旧 Agent 提示升级。
- Agent 的 `realm.v1` 管理能力与 `realm.traffic.v1` 可选统计能力；仅接管安装器管理的服务，首次备份、原子替换、失败回滚、启停与开机启动状态恢复。
- Hub 数据库当前版本 24（最初 Realm 管理版本为 22）；Realm 配置、成功应用快照和修订号持久化。旧结果不能发布新配置，备份恢复后远端更高修订会自动重新基线化。
- 用户中心、Clash 与 sing-box 订阅追加入口；继承目标代理授权和认证，不重复计费。停用或撤销授权立即撤回；目标地址变化不继续发布过期入口。
- 引用删除保护、监听端口冲突、目标变更自动同步；管理接口沿用鉴权及跨站防护。

## 已验证

- Hub：fmt、Clippy（warnings 为错误）、225 项测试及 Linux debug build。
- Agent：fmt、Clippy、57 项测试及 Linux debug build。测试容器安装 systemd-detect-virt，未跳过现有环境交叉校验。
- 原阶段前端：lint、六组测试和 TypeScript/Vite 构建；当时 Realm 页面单独加载。当前代理整合验收见 `REALM_PROXY_UI.md`。
- 安装脚本：Realm 生命周期、Agent unit、API 端口选择检查。
- 真实联调：官方 Realm v2.9.6 aarch64 包的 SHA-256 与安装器固定摘要一致。隔离容器内启动新 Hub、新 Agent 和真实 Realm，通过 API 保存规则后自动启动并完成真实 TCP echo 转发；测试用户的用户中心、Clash/sing-box 出现额外入口，停用后立即撤回且确认服务停止。
- systemd/OpenRC 命令及回滚通过适配器测试；真实联调中的 systemctl 使用测试适配器，不代表在真实 init 主机上的验收。

## 未完成的环境验收及发布事项

- 原阶段浏览器登录跳转后自动化连接失效，当时未完成表单逐项交互验收。当前代理整合的独立浏览器验收记录见 `REALM_PROXY_UI.md`，不代表真实 Agent 或生产部署验收。
- 真实多机器 VLESS Reality 认证链路、真实 systemd/OpenRC 主机部署未验收；实际字节转发测试使用本地 TCP echo 目标。
- 发布核对发现已有不匹配：Hub 使用上游固定公钥验签，fork Agent 发布流程未生成 `.sig`。未移除或绕过验签；签名发布须另行接通，或暂只交付源码并在受控环境安装构建产物。未将源码 push 当作二进制已发布或生产已部署。


## 交付时远程状态

本地功能与复核修复已分别提交。Hub SSH 22/443 推送连接超时，HTTPS 回退无可用凭据；只读核对远程 dev 仍为 `911e63a`，未将本地提交误报为已推送。Agent 未创建发布 tag，避免在签名配套未确定前发布安装链路无法验签的新包。
