import { api } from "@/lib/api"

export type AddressType = "domain" | "ipv4" | "ipv6"
export type Flow = "" | "xtls-rprx-vision"

export type RealityConfig = {
  enabled: boolean
  server_name: string
  server_port: number
  private_key: string
  public_key: string
  short_id: string
}

export type Proxy = {
  id: number
  node_id: number
  name: string
  include_node_name: boolean
  protocol: "vless"
  address_type: AddressType
  address: string
  port: number
  enabled: boolean
  config: { reality: RealityConfig }
  created_at: number
  updated_at: number
}

export type ProxyDraft = Omit<Proxy, "id" | "node_id" | "created_at" | "updated_at">

export type User = {
  id: number
  username: string
  uuid: string
  enabled: boolean
  expires_at: number | null
  created_at: number
  updated_at: number
  /** Number of assigned proxy records, including disabled authorizations. */
  proxy_count: number
}

export type UserDraft = Pick<User, "username" | "enabled" | "expires_at"> & { password?: string }
export type VlessAuth = { flow: Flow }
export type UserProxyAuthorization = {
  proxy: Pick<Proxy, "id" | "node_id" | "name" | "include_node_name" | "protocol" | "address_type" | "address" | "port" | "enabled">
  access: { enabled: boolean; auth: VlessAuth }
}
export type AccessDraft = { enabled: boolean; auth: VlessAuth }

export async function listProxiesForNode(nodeId: number): Promise<Proxy[]> {
  const response = await api<{ items: Proxy[] }>(`/nodes/${nodeId}/proxies`)
  return response.items
}

export async function listAllProxies(nodeIds: number[]): Promise<Proxy[]> {
  const responses = await Promise.all(nodeIds.map(listProxiesForNode))
  return responses.flat().sort((a, b) => a.node_id - b.node_id || a.id - b.id)
}

export function createProxy(nodeId: number, draft: ProxyDraft) {
  return api<Proxy>(`/nodes/${nodeId}/proxies`, { method: "POST", body: JSON.stringify(draft) })
}

export function updateProxy(id: number, draft: ProxyDraft, nodeId?: number) {
  const body = nodeId === undefined ? draft : { ...draft, node_id: nodeId }
  return api<Proxy>(`/proxies/${id}`, { method: "PUT", body: JSON.stringify(body) })
}

export function deleteProxy(id: number) {
  return api<void>(`/proxies/${id}`, { method: "DELETE" })
}

export async function listUsers(): Promise<User[]> {
  const response = await api<{ items: User[] }>("/users")
  return response.items
}

export function createUser(draft: UserDraft) {
  return api<User>("/users", { method: "POST", body: JSON.stringify(draft) })
}

export function updateUser(id: number, draft: UserDraft) {
  return api<User>(`/users/${id}`, { method: "PUT", body: JSON.stringify(draft) })
}

export function resetUserUuid(id: number) {
  return api<{ uuid: string; queued_node_ids: number[]; needs_sync_node_ids: number[] }>(`/users/${id}/uuid`, { method: "POST" })
}

export function deleteUser(id: number) {
  return api<void>(`/users/${id}`, { method: "DELETE" })
}

export async function listUserAuthorizations(userId: number): Promise<UserProxyAuthorization[]> {
  const response = await api<{ items: UserProxyAuthorization[] }>(`/users/${userId}/proxies`)
  return response.items
}

export function saveUserProxy(userId: number, proxyId: number, draft: AccessDraft) {
  return api<{ user_id: number; proxy_id: number; enabled: boolean; auth: VlessAuth }>(
    `/users/${userId}/proxies/${proxyId}`,
    { method: "PUT", body: JSON.stringify(draft) },
  )
}

export function deleteUserProxy(userId: number, proxyId: number) {
  return api<void>(`/users/${userId}/proxies/${proxyId}`, { method: "DELETE" })
}
