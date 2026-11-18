import { useState } from "react"
import { toast } from "sonner"
import type { Node } from "@/lib/api"
import { createProxy, updateProxy, type Proxy, type ProxyDraft, type AddressType } from "@/lib/resources"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog"

export function Hysteria2Form({ proxy, nodes, onClose, onSaved, onBack, embedded, onBusyChange }: {
  proxy: Proxy | null; nodes: Node[]; onClose: () => void; onSaved: () => void;
  onBack?: () => void; embedded?: boolean; onBusyChange?: (busy: boolean) => void
}) {
  const config = proxy?.config.hysteria2
  const [node, setNode] = useState(proxy?.node_id ?? nodes[0]?.id ?? 0)
  const [name, setName] = useState(proxy?.name ?? "HY2")
  const [address, setAddress] = useState(proxy?.address ?? "")
  const [addressType, setAddressType] = useState<AddressType>(proxy?.address_type ?? "domain")
  const [port, setPort] = useState(proxy?.port ?? 443)
  const [sni, setSni] = useState(config?.server_name ?? "www.example.com")
  const [obfs, setObfs] = useState(config?.obfs_password ?? "")
  const [up, setUp] = useState(config?.up_mbps ?? 100)
  const [down, setDown] = useState(config?.down_mbps ?? 100)
  const [busy, setBusy] = useState(false)
  const form = <form className="space-y-4 p-6" onSubmit={async event => {
    event.preventDefault(); setBusy(true); onBusyChange?.(true)
    try {
      const draft: ProxyDraft = { name, address, address_type: addressType, port, enabled: proxy?.enabled ?? true,
        include_node_name: proxy?.include_node_name ?? false, protocol: "hysteria2", flow: "",
        config: { hysteria2: { server_name: sni, obfs_password: obfs, up_mbps: up, down_mbps: down } } }
      const saved = proxy ? await updateProxy(proxy.id, draft, node) : await createProxy(node, draft)
      if (saved.sync?.needs_sync?.length) toast.warning("已保存，节点配置仍需同步")
      else toast.success("HY2 已保存，请确认节点同步结果")
      onSaved(); onClose()
    } catch (error) { toast.error((error as Error).message) }
    finally { setBusy(false); onBusyChange?.(false) }
  }}>
    <DialogHeader><DialogTitle>{proxy ? "编辑 HY2" : "新增 HY2"}</DialogTitle></DialogHeader>
    <p className="text-sm text-muted-foreground">使用节点 sing-box 和自动自签证书，连接密码为用户 UUID。请放行 UDP 端口。</p>
    <label className="block">节点<select aria-label="节点" className="block w-full border p-2" value={node} onChange={e => setNode(Number(e.target.value))}>{nodes.map(n => <option key={n.id} value={n.id}>{n.name}</option>)}</select></label>
    <label className="block">名称<Input required maxLength={128} value={name} onChange={e => setName(e.target.value)} /></label>
    <label className="block">地址类型<select aria-label="地址类型" className="block w-full border p-2" value={addressType} onChange={e => setAddressType(e.target.value as AddressType)}><option value="domain">域名</option><option value="ipv4">IPv4</option><option value="ipv6">IPv6</option></select></label>
    <label className="block">连接地址<Input required value={address} onChange={e => setAddress(e.target.value)} /></label>
    <label className="block">UDP 端口<Input required type="number" min={1} max={65535} value={port} onChange={e => setPort(Number(e.target.value))} /></label>
    <label className="block">SNI<Input required value={sni} onChange={e => setSni(e.target.value)} /></label>
    <label className="block">Salamander 密码（留空关闭混淆）<Input type="password" autoComplete="new-password" value={obfs} onChange={e => setObfs(e.target.value)} /></label>
    <label className="block">客户端上行 Mbps<Input required type="number" min={1} value={up} onChange={e => setUp(Number(e.target.value))} /></label>
    <label className="block">客户端下行 Mbps<Input required type="number" min={1} value={down} onChange={e => setDown(Number(e.target.value))} /></label>
    <div className="flex gap-2">{onBack && <Button type="button" variant="outline" disabled={busy} onClick={onBack}>返回</Button>}<Button type="button" variant="outline" disabled={busy} onClick={onClose}>取消</Button><Button disabled={busy || !node}>保存</Button></div>
  </form>
  return embedded ? form : <Dialog open onOpenChange={open => !open && !busy && onClose()}><DialogContent className="max-h-[90vh] overflow-y-auto">{form}</DialogContent></Dialog>
}
