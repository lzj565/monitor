import { displayProxyName } from "./proxy-name.ts"
import type { Node } from "./api"
import type { Proxy } from "./resources"

export type ProxyListItem = { kind: "proxy"; key: string; proxy: Proxy }
export type ProxyTypeFilter = "all" | "vless" | "hysteria2" | "snell"

export function proxyTypeFilter(search: string): ProxyTypeFilter {
  const type = new URLSearchParams(search).get("type")
  return type === "snell" || type === "hysteria2" || type === "vless" ? type : "all"
}
export function proxyListItems(proxies: Proxy[], nodes: Node[], filters: { query: string; node: string; status: string; type: ProxyTypeFilter }): ProxyListItem[] {
  const nodeById = new Map(nodes.map((n) => [n.id, n]))
  const items: ProxyListItem[] = [
    ...proxies.map((proxy): ProxyListItem => ({ kind: "proxy", key: `proxy:${proxy.id}`, proxy })),
  ]
  const needle = filters.query.trim().toLowerCase()
  return items.filter((item) => {
    const row = item.proxy
    if (filters.type !== "all" && filters.type !== item.proxy.protocol) return false
    if (filters.node !== "all" && filters.node !== String(row.node_id)) return false
    if (filters.status !== "all" && (filters.status === "enabled") !== row.enabled) return false
    const values = [displayProxyName(item.proxy, nodeById.get(row.node_id)), row.name, item.proxy.address, String(item.proxy.port), nodeById.get(row.node_id)?.country]
    return !needle || [...values, nodeById.get(row.node_id)?.name].some((v) => v?.toLowerCase().includes(needle))
  })
}
