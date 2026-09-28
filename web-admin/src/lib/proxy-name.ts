import type { Node } from "@/lib/api"

type NamedProxy = { name: string; include_node_name: boolean; node_id: number }

export function countryFlag(country: string | undefined): string {
  if (!country || !/^[a-z]{2}$/i.test(country)) return ""
  return String.fromCodePoint(...[...country.toUpperCase()].map((letter) => 0x1f1e6 + letter.charCodeAt(0) - 65))
}

export function displayProxyName(proxy: NamedProxy, node?: Pick<Node, "name" | "country">): string {
  if (!proxy.include_node_name) return proxy.name.trim()
  const country = node?.country?.trim().toUpperCase() ?? ""
  if (!/^[A-Z]{2}$/.test(country)) return proxy.name.trim()
  return `${countryFlag(country)}${country}-${proxy.name.trim()}`
}
