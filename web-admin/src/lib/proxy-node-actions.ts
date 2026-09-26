import type { ProxyNode } from "./api.ts"

export type ProxyNodeUpdatePayload = Pick<
  ProxyNode,
  "name" | "enabled" | "address_mode" | "custom_address" | "listen_port" | "reality_server_name" | "reality_dest"
>

export type ProxyNodeCredential = "uuid" | "reality_key" | "short_id"
export type ProxyNodeActionApi = <T = unknown>(path: string, init?: RequestInit) => Promise<T>
export type ProxyNodeShare = { node_id: number; name: string; address: string; uri: string }

export async function fetchProxyNodeShare(
  request: ProxyNodeActionApi,
  id: number,
  onLoading?: (loading: boolean) => void,
): Promise<ProxyNodeShare> {
  onLoading?.(true)
  try {
    return await request<ProxyNodeShare>(`/proxy/nodes/${id}/share`, { cache: "no-store" })
  } finally {
    onLoading?.(false)
  }
}

export function proxyNodeShareDisabledReason(): string {
  return "节点分享链接已停用，请在用户管理中创建代理用户并授权节点"
}

export function proxyNodeUpdatePayload(
  node: ProxyNodeUpdatePayload,
  changes: Partial<ProxyNodeUpdatePayload> = {},
): ProxyNodeUpdatePayload {
  return {
    name: node.name,
    enabled: node.enabled,
    address_mode: node.address_mode,
    custom_address: node.custom_address,
    listen_port: node.listen_port,
    reality_server_name: node.reality_server_name,
    reality_dest: node.reality_dest,
    ...changes,
  }
}

export async function updateAndDeployProxyNode(
  request: ProxyNodeActionApi,
  id: number,
  nodeId: number,
  update: ProxyNodeUpdatePayload,
  onUpdated?: () => void,
  onDeployed?: () => void,
): Promise<void> {
  await request(`/proxy/nodes/${id}`, { method: "PUT", body: JSON.stringify(update) })
  onUpdated?.()
  await request(`/proxy/servers/${nodeId}/deploy`, { method: "POST" })
  onDeployed?.()
}

export async function regenerateAndDeployProxyNode(
  request: ProxyNodeActionApi,
  id: number,
  nodeId: number,
  credential: ProxyNodeCredential,
  onRegenerated?: (node: ProxyNode) => void,
  onDeployed?: () => void,
): Promise<ProxyNode> {
  const result = await request<{ node: ProxyNode }>(`/proxy/nodes/${id}/regenerate`, {
    method: "POST",
    body: JSON.stringify({ credential }),
  })
  onRegenerated?.(result.node)
  await request(`/proxy/servers/${nodeId}/deploy`, { method: "POST" })
  onDeployed?.()
  return result.node
}

export async function removeProxyNode(request: ProxyNodeActionApi, id: number): Promise<void> {
  await request(`/proxy/nodes/${id}`, { method: "DELETE" })
}

export function regenerateConfirmation(credential: ProxyNodeCredential, name?: string): { title: string; description: string; action: string } {
  const forNode = name ? `节点「${name}」` : "当前"
  switch (credential) {
    case "uuid":
      return {
        title: "重新生成 UUID？",
        description: `${forNode}使用旧 UUID 的客户端配置将无法继续连接。`,
        action: "重新生成 UUID",
      }
    case "reality_key":
      return {
        title: "重新生成 Reality 密钥？",
        description: `${forNode}使用旧 Reality 密钥的客户端配置将无法继续连接。`,
        action: "重新生成 Reality 密钥",
      }
    case "short_id":
      return {
        title: "重新生成 Short ID？",
        description: `${forNode}使用旧 Short ID 的客户端配置将无法继续连接。`,
        action: "重新生成 Short ID",
      }
  }
}

export function deleteConfirmation(name: string): { title: string; description: string; action: string } {
  return {
    title: `删除代理节点「${name}」？`,
    description: "将从服务器的 sing-box 配置中移除此代理节点。删除后相关客户端连接将立即失效。",
    action: "删除",
  }
}

export function createProxyNodeOperationGuard() {
  const busy = new Set<number>()
  return {
    tryStart(id: number) {
      if (busy.has(id)) return false
      busy.add(id)
      return true
    },
    finish(id: number) {
      busy.delete(id)
    },
  }
}
