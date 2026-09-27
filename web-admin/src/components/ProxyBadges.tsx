import { Badge } from "@/components/ui/badge"

export function ProxyProtocolBadge({ protocol = "vless" }: { protocol?: string }) {
  const label = protocol === "vless" ? "VLESS + Reality" : protocol
  return <Badge variant="outline" className="border-violet-500/35 bg-violet-500/10 font-medium text-violet-700 dark:text-violet-300">{label}</Badge>
}

export function ProxyAddressTypeBadge({ addressType }: { addressType: string }) {
  const label = addressType === "domain" ? "自定义" : addressType.toUpperCase()
  return <Badge variant="outline" className={addressType === "domain" ? "font-normal" : "border-emerald-500/40 bg-emerald-500/10 font-normal text-emerald-700 dark:text-emerald-300"}>{label}</Badge>
}
