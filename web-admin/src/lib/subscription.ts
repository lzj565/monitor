import type { Node } from "@/lib/api"
import { displayProxyName } from "./proxy-name.ts"
import type { Proxy, UserProxyAuthorization } from "@/lib/resources"

export type ProxyNodeGroup = { node: Node | undefined; nodeId: number; proxies: Proxy[] }

export function groupProxiesByNode(proxies: Proxy[], nodes: Node[]): ProxyNodeGroup[] {
  const nodeOrder = new Map(nodes.map((node, index) => [node.id, index]))
  const groups = new Map<number, ProxyNodeGroup>()
  for (const proxy of proxies) {
    const group = groups.get(proxy.node_id) ?? {
      node: nodes.find((node) => node.id === proxy.node_id),
      nodeId: proxy.node_id,
      proxies: [],
    }
    group.proxies.push(proxy)
    groups.set(proxy.node_id, group)
  }
  return [...groups.values()].sort(
    (a, b) => (nodeOrder.get(a.nodeId) ?? Number.MAX_SAFE_INTEGER) - (nodeOrder.get(b.nodeId) ?? Number.MAX_SAFE_INTEGER) || a.nodeId - b.nodeId,
  )
}

export function proxyGroupSelection(selectedIds: Iterable<number>, proxyIds: number[]): { selected: number; total: number; checked: boolean; indeterminate: boolean } {
  const selected = new Set(selectedIds)
  const count = proxyIds.reduce((sum, id) => sum + Number(selected.has(id)), 0)
  return { selected: count, total: proxyIds.length, checked: proxyIds.length > 0 && count === proxyIds.length, indeterminate: count > 0 && count < proxyIds.length }
}

export function toggleProxyGroup(selectedIds: Iterable<number>, proxyIds: number[]): number[] {
  const next = new Set(selectedIds)
  const { checked } = proxyGroupSelection(next, proxyIds)
  for (const id of proxyIds) {
    if (checked) next.delete(id)
    else next.add(id)
  }
  return [...next]
}

export function subscriptionSearchMatches(
  username: string,
  accesses: UserProxyAuthorization[],
  proxiesById: ReadonlyMap<number, Proxy>,
  nodesById: ReadonlyMap<number, Node>,
  query: string,
): boolean {
  const needle = query.trim().toLocaleLowerCase()
  if (!needle || username.toLocaleLowerCase().includes(needle)) return true
  return accesses.some(({ proxy: accessProxy }) => {
    const proxy = proxiesById.get(accessProxy.id) ?? accessProxy
    const node = nodesById.get(accessProxy.node_id)
    return [displayProxyName(proxy, node), proxy.name, node?.name, proxy.address].some((value) => value?.toLocaleLowerCase().includes(needle))
  })
}

export function existingAuthorizationSettings(accesses: UserProxyAuthorization[]): Record<number, { flow: "" | "xtls-rprx-vision"; enabled: boolean }> {
  return Object.fromEntries(accesses.map(({ proxy, access }) => [proxy.id, { flow: access.auth.flow, enabled: access.enabled }]))
}
