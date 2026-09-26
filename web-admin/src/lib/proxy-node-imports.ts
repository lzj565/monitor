export type ProxyNodeImportCandidate = {
  source_tag: string | null
  importable: boolean
  reason: string | null
  name: string
  protocol: string
  listen_address: string | null
  listen_port: number | null
  uuid: string | null
  reality_public_key: string | null
  reality_short_id: string | null
  reality_server_name: string | null
  reality_dest: string | null
  suggested_address_mode: "ipv4" | "ipv6" | null
  suggested_address: string | null
}

export type ProxyNodeImportScan = {
  config_fingerprint: string
  inbounds: ProxyNodeImportCandidate[]
}

export type ProxyNodeImportRequest = {
  config_fingerprint: string
  source_tag: string
  name: string
  address_mode: "ipv4" | "ipv6" | "custom"
  custom_address: string | null
}

export type ProxyNodeImportApi = <T = unknown>(path: string, init?: RequestInit) => Promise<T>

export async function scanProxyNodeImports(
  request: ProxyNodeImportApi,
  nodeId: number,
  onLoading?: (loading: boolean) => void,
): Promise<ProxyNodeImportScan> {
  onLoading?.(true)
  try {
    return await request<ProxyNodeImportScan>(`/proxy/servers/${nodeId}/imports`, { cache: "no-store" })
  } finally {
    onLoading?.(false)
  }
}

export async function importProxyNodeInbound(
  request: ProxyNodeImportApi,
  nodeId: number,
  payload: ProxyNodeImportRequest,
  onLoading?: (loading: boolean) => void,
): Promise<void> {
  onLoading?.(true)
  try {
    await request(`/proxy/servers/${nodeId}/imports`, {
      method: "POST",
      body: JSON.stringify(payload),
    })
  } finally {
    onLoading?.(false)
  }
}
