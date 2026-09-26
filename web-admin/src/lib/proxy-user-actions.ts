import type { ProxyUser } from "./api.ts"

export type ProxyUserInput = Omit<Pick<
  ProxyUser,
  "name" | "enabled" | "note" | "proxy_node_ids" | "traffic_limit_bytes" | "traffic_reset_day" | "expire_date"
>, "expire_date"> & { expire_date?: string | null }
export type ProxyUserResult = { user: ProxyUser; failed_servers: { server_id: number; server_name: string; error: string }[] }
export type ProxyUserDeleteResult = { deleted: boolean; user: ProxyUser | null; failed_servers: ProxyUserResult["failed_servers"] }
export type ProxyUserActionApi = <T = unknown>(path: string, init?: RequestInit) => Promise<T>

export function saveProxyUser(
  request: ProxyUserActionApi,
  id: number | null,
  input: ProxyUserInput,
): Promise<ProxyUserResult> {
  return request<ProxyUserResult>(id === null ? "/proxy/users" : `/proxy/users/${id}`, {
    method: id === null ? "POST" : "PUT",
    body: JSON.stringify(input),
  })
}

export function regenerateProxyUser(request: ProxyUserActionApi, id: number): Promise<ProxyUserResult> {
  return request<ProxyUserResult>(`/proxy/users/${id}/regenerate`, { method: "POST" })
}

export function deleteProxyUser(request: ProxyUserActionApi, id: number): Promise<ProxyUserDeleteResult> {
  return request<ProxyUserDeleteResult>(`/proxy/users/${id}`, { method: "DELETE" })
}

export function syncProxyUser(request: ProxyUserActionApi, id: number): Promise<ProxyUserResult> {
  return request<ProxyUserResult>(`/proxy/users/${id}/sync`, { method: "POST" })
}

export function resetProxyUserTraffic(request: ProxyUserActionApi, id: number): Promise<ProxyUserResult> {
  return request<ProxyUserResult>(`/proxy/users/${id}/traffic/reset`, { method: "POST" })
}

export function proxyUserRegenerateConfirmation(name: string) {
  return {
    title: `重新生成「${name}」的 UUID？`,
    description: "重新生成 UUID 后，该用户现有客户端配置将失效。",
    action: "重新生成 UUID",
  }
}

export function proxyUserDeleteConfirmation(name: string) {
  return {
    title: `删除代理用户「${name}」？`,
    description: "Monitor 会先从已授权节点的 sing-box 配置中移除此用户。全部服务器同步成功后才会删除用户记录。",
    action: "删除用户",
  }
}

export function createProxyUserOperationGuard() {
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
