import { displayProxyName } from "./proxy-name.ts"
import type { Node } from "./api"
import type { Proxy } from "./resources"

export type RealmHost = { node_id: number; name: string; public_address: string; enabled: boolean; desired_revision: number; applied_revision: number; pending: boolean; online: boolean; capable: boolean; traffic_capable: boolean; last_error: string; status: { installed?: boolean; managed?: boolean; version?: string; running?: boolean; enabled?: boolean; target_reachability?: string; targets_checked?: number; targets_total?: number } }
export type RealmRule = { traffic_key: string; id: number; node_id: number; target_proxy_id: number; name: string; listen_host: string; listen_port: number; enabled: boolean; target_node_id: number; target_name: string; target_address: string; target_port: number; target_enabled: boolean }
export type RealmRuleDraft = Pick<RealmRule, "node_id" | "target_proxy_id" | "name" | "listen_host" | "listen_port" | "enabled">
export type RealmHostDraft = { node_id: number; public_address: string; enabled: boolean; takeover: boolean }
export type ProxyListItem = { kind: "proxy"; key: string; proxy: Proxy } | { kind: "realm"; key: string; rule: RealmRule }
export type ProxyTypeFilter = "all" | "vless" | "realm"

export function proxyTypeFilter(search: string): ProxyTypeFilter {
  const type = new URLSearchParams(search).get("type")
  return type === "realm" || type === "vless" ? type : "all"
}
export function realmHostStatus(host?: RealmHost): string {
  if (!host) return "等待入口配置"
  if (!host.online) return "节点离线，等待重连"
  if (!host.capable) return "需要升级 Agent"
  if (host.status.installed === false) return "未安装 Realm"
  if (host.status.managed === false) return "不是本安装器管理的 Realm"
  if (host.last_error) return "应用失败 / 等待重试"
  if (host.pending) return "待应用"
  if (!host.enabled) return "入口服务已停用"
  return host.status.running ? "运行中" : host.status.running === false ? "已停止" : "读取状态中"
}
export function proxyListItems(proxies: Proxy[], rules: RealmRule[], hosts: RealmHost[], nodes: Node[], filters: { query: string; node: string; status: string; type: ProxyTypeFilter }): ProxyListItem[] {
  const nodeById = new Map(nodes.map((n) => [n.id, n]))
  const hostById = new Map(hosts.map((h) => [h.node_id, h]))
  const items: ProxyListItem[] = [
    ...proxies.map((proxy): ProxyListItem => ({ kind: "proxy", key: `proxy:${proxy.id}`, proxy })),
    ...[...rules].sort((a, b) => a.node_id - b.node_id || a.id - b.id).map((rule): ProxyListItem => ({ kind: "realm", key: `realm:${rule.id}`, rule })),
  ]
  const needle = filters.query.trim().toLowerCase()
  return items.filter((item) => {
    const row = item.kind === "proxy" ? item.proxy : item.rule
    if (filters.type !== "all" && filters.type !== (item.kind === "proxy" ? "vless" : "realm")) return false
    if (filters.node !== "all" && filters.node !== String(row.node_id)) return false
    if (filters.status !== "all" && (filters.status === "enabled") !== row.enabled) return false
    const values = item.kind === "proxy"
      ? [displayProxyName(item.proxy, nodeById.get(row.node_id)), row.name, item.proxy.address, String(item.proxy.port), nodeById.get(row.node_id)?.country]
      : [row.name, hostById.get(row.node_id)?.public_address, String(item.rule.listen_port), item.rule.target_name, item.rule.target_address, nodeById.get(item.rule.target_node_id)?.name]
    return !needle || [...values, nodeById.get(row.node_id)?.name].some((v) => v?.toLowerCase().includes(needle))
  })
}

/** Save shared settings only when changed. A failed rule save leaves an idle entry
 * configuration; retrying never creates a second rule or removes shared settings. */
export async function saveRealmRule(
  request: <T>(path: string, init: { method: string; body: string }) => Promise<T>,
  id: number | null, rule: RealmRuleDraft, entry: RealmHostDraft, previous?: RealmHost,
): Promise<{ id: number }> {
  if (rule.node_id !== entry.node_id) throw new Error("入口节点不一致")
  if (!previous || previous.public_address !== entry.public_address || previous.enabled !== entry.enabled) {
    await request("/realm/hosts", { method: "PUT", body: JSON.stringify(entry) })
  }
  return request(id === null ? "/realm/rules" : `/realm/rules/${id}`, { method: id === null ? "POST" : "PUT", body: JSON.stringify(rule) })
}
