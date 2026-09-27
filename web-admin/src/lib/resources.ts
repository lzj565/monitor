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

export type ProxyTrafficSummary = {
  user_id?: number
  node_id: number
  node_name: string
  username?: string
  uplink_bytes: number
  downlink_bytes: number
  last_seen_at: number | null
  reset_at: number | null
}

export type ProxyTrafficOverview = {
  uplink_bytes: number
  downlink_bytes: number
  total_bytes: number
  active_users: number
}

export type ProxyTrafficUserRow = {
  user_id: number
  username: string
  node_id: number
  node_name: string
  uplink_bytes: number
  downlink_bytes: number
  total_bytes: number
  last_seen_at: number | null
}

export type ProxyTrafficNodeRow = {
  node_id: number
  node_name: string
  protocols: string[]
  uplink_bytes: number
  downlink_bytes: number
  total_bytes: number
  last_seen_at: number | null
}

export type ProxyTrafficPage<T> = {
  items: T[]
  page: number
  page_size: number
  total: number
}

export type ProxyTrafficListOptions = {
  q: string
  node_id: number | null
  sort: "total" | "uplink" | "downlink"
  order: "asc" | "desc"
  page: number
  page_size: number
}

function trafficSearch(options: ProxyTrafficListOptions): string {
  const query = new URLSearchParams({
    sort: options.sort,
    order: options.order,
    page: String(options.page),
    page_size: String(options.page_size),
  })
  if (options.q.trim()) query.set("q", options.q.trim())
  if (options.node_id !== null) query.set("node_id", String(options.node_id))
  return query.toString()
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

export async function listProxyUserTraffic(): Promise<Array<Pick<ProxyTrafficSummary, "user_id" | "username" | "uplink_bytes" | "downlink_bytes" | "last_seen_at" | "reset_at">>> {
  const response = await api<{ items: Array<Pick<ProxyTrafficSummary, "user_id" | "username" | "uplink_bytes" | "downlink_bytes" | "last_seen_at" | "reset_at">> }>("/proxy-traffic/users")
  return response.items
}

export async function getProxyUserTraffic(userId: number): Promise<ProxyTrafficSummary[]> {
  const response = await api<{ items: ProxyTrafficSummary[] }>(`/proxy-traffic/users/${userId}`)
  return response.items
}

export async function listProxyNodeTraffic(): Promise<ProxyTrafficSummary[]> {
  const response = await api<{ items: ProxyTrafficSummary[] }>("/proxy-traffic/nodes")
  return response.items
}

export function getProxyTrafficOverview() {
  return api<ProxyTrafficOverview>("/proxy-traffic/summary")
}

export function listProxyTrafficUserRows(options: ProxyTrafficListOptions) {
  return api<ProxyTrafficPage<ProxyTrafficUserRow>>(`/proxy-traffic/user-rows?${trafficSearch(options)}`)
}

export function listProxyTrafficNodeRows(options: ProxyTrafficListOptions) {
  return api<ProxyTrafficPage<ProxyTrafficNodeRow>>(`/proxy-traffic/node-rows?${trafficSearch(options)}`)
}

export function resetProxyUserTraffic(userId: number) {
  return api<void>(`/proxy-traffic/users/${userId}/reset`, { method: "POST" })
}

export function resetProxyNodeTraffic(nodeId: number) {
  return api<void>(`/proxy-traffic/nodes/${nodeId}/reset`, { method: "POST" })
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
