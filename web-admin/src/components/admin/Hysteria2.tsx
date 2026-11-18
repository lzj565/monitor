import { ProxyAddressField, defaultAddressType, nodeAddress } from "../ProxyAddressField"
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
  const selectedNode = nodes.find(candidate => candidate.id === node)
  const [customAddress, setCustomAddress] = useState(proxy?.address_type === "domain" ? proxy.address : "")
  const [addressType, setAddressType] = useState<AddressType>(proxy?.address_type ?? defaultAddressType(selectedNode))
  const address = addressType === "domain"
    ? customAddress
    : nodeAddress(selectedNode, addressType) || (proxy?.address_type === addressType ? proxy.address : "")

  function changeNode(nextId: number) {
    const nextNode = nodes.find(candidate => candidate.id === nextId)
    setNode(nextId)
    if (addressType !== "domain" && !nodeAddress(nextNode, addressType)) {
      setAddressType(defaultAddressType(nextNode))
    }
  }

  const [port, setPort] = useState(proxy?.port ?? 443)
  const [sni, setSni] = useState(config?.server_name ?? "www.amd.com")
  const [obfs, setObfs] = useState(() => proxy
    ? config?.obfs_password ?? ""
    : Array.from(crypto.getRandomValues(new Uint8Array(16)), byte => byte.toString(16).padStart(2, "0")).join(""))
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
    <label className="block">节点<select aria-label="节点" className="block w-full border p-2" value={node} onChange={e => changeNode(Number(e.target.value))}>{nodes.map(n => <option key={n.id} value={n.id}>{n.name}</option>)}</select></label>
    <label className="block">名称<Input required maxLength={128} value={name} onChange={e => setName(e.target.value)} /></label>
    <ProxyAddressField selectedNode={selectedNode} addressType={addressType} address={address} changeAddressType={setAddressType} setCustomAddress={setCustomAddress} />
    <label className="block">UDP 端口<Input required type="number" min={1} max={65535} value={port} onChange={e => setPort(Number(e.target.value))} /></label>
    <label className="block">SNI<Input required value={sni} onChange={e => setSni(e.target.value)} /><span className="text-xs text-muted-foreground">用于生成自签证书，可直接使用默认域名。</span></label>
    <label className="block">Salamander 密码<Input type="password" autoComplete="new-password" value={obfs} onChange={e => setObfs(e.target.value)} /><span className="text-xs text-muted-foreground">客户端与服务端使用同一个混淆密码，留空关闭混淆。</span></label>
    <label className="block">客户端上行 Mbps<Input required type="number" min={1} value={up} onChange={e => setUp(Number(e.target.value))} /></label>
    <label className="block">客户端下行 Mbps<Input required type="number" min={1} value={down} onChange={e => setDown(Number(e.target.value))} /></label>
    <div className="flex gap-2">{onBack && <Button type="button" variant="outline" disabled={busy} onClick={onBack}>返回</Button>}<Button type="button" variant="outline" disabled={busy} onClick={onClose}>取消</Button><Button disabled={busy || !node}>保存</Button></div>
  </form>
  return embedded ? form : <Dialog open onOpenChange={open => !open && !busy && onClose()}><DialogContent className="max-h-[90vh] overflow-y-auto">{form}</DialogContent></Dialog>
}
