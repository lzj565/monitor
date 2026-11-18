import { useId } from "react"
import type { Node } from "@/lib/api"
import type { ProxyDraft } from "@/lib/resources"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

export function nodeAddress(node: Node | undefined, type: "ipv4" | "ipv6"): string {
  return node?.addresses?.find(({ address }) => (type === "ipv6") === address.includes(":"))?.address ?? ""
}

export function defaultAddressType(node: Node | undefined): ProxyDraft["address_type"] {
  if (nodeAddress(node, "ipv4")) return "ipv4"
  if (nodeAddress(node, "ipv6")) return "ipv6"
  return "domain"
}

export function ProxyAddressField({ selectedNode, addressType, address, changeAddressType, setCustomAddress }: {
  selectedNode: Node | undefined
  addressType: ProxyDraft["address_type"]
  address: string
  changeAddressType: (type: ProxyDraft["address_type"]) => void
  setCustomAddress: (address: string) => void
}) {
  const id = useId()
  return (
    <section className="space-y-2 border-t pt-3">
      <div className="space-y-2">
        <Label htmlFor={id}>连接地址 *</Label>
        <div className="mb-2 flex gap-1">
          {([["ipv4", "IPv4"], ["ipv6", "IPv6"], ["domain", "自定义"]] as const).map(([type, label]) => <Button key={type} type="button" size="sm" variant={addressType === type ? "default" : "outline"} disabled={type !== "domain" && !nodeAddress(selectedNode, type)} aria-pressed={addressType === type} onClick={() => changeAddressType(type)}>{label}</Button>)}
        </div>
        <div className="flex gap-2">
          <Input
            id={id}
            required
            readOnly={addressType !== "domain"}
            value={address}
            onChange={(event) => setCustomAddress(event.target.value)}
            placeholder={addressType === "domain" ? "hk.example.com 或 IP 地址" : "节点尚未上报此地址"}
          />
          <Badge variant="outline" className={`h-9 shrink-0 px-3 ${addressType === "domain" ? "" : "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"}`}>
            {addressType === "domain" ? "自定义" : addressType.toUpperCase()}
          </Badge>
        </div>
        {addressType !== "domain" && <p className="text-xs text-muted-foreground">自动采用所属节点的可连接地址。</p>}
      </div>
    </section>
  )
}
