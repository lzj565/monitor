import type { Proxy, ProxyDraft } from "./resources.ts"

/** A complete PUT draft based on a listed proxy, optionally changing its enabled state. */
export function proxyDraft(proxy: Proxy, enabled = proxy.enabled): ProxyDraft {
  return {
    name: proxy.name,
    include_node_name: proxy.include_node_name,
    protocol: proxy.protocol,
    address_type: proxy.address_type,
    address: proxy.address,
    port: proxy.port,
    enabled,
    config: proxy.config,
  }
}
