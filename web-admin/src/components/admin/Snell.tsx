import { ProxyAddressField, defaultAddressType, nodeAddress } from "../ProxyAddressField"
import { useState } from "react"
import { toast } from "sonner"
import type { Node } from "@/lib/api"
import { createProxy, updateProxy, type Proxy, type ProxyDraft, type AddressType } from "@/lib/resources"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ProxyDialog } from "@/components/ProxyDialog"
import { DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"

export function SnellForm({ proxy, nodes, onClose, onSaved, onBack, embedded, onBusyChange }: {
  proxy: Proxy | null; nodes: Node[]; onClose: () => void; onSaved: () => void;
  onBack?: () => void; embedded?: boolean; onBusyChange?: (busy: boolean) => void
}) {
  const [node, setNode] = useState(proxy?.node_id ?? nodes[0]?.id ?? 0)
  const [name, setName] = useState(proxy?.name ?? "Snell v6")
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
  const [busy, setBusy] = useState(false)
  const form = <form className="flex min-h-0 flex-1 flex-col" onSubmit={async event => {
    event.preventDefault(); setBusy(true); onBusyChange?.(true)
    try {
      const draft: ProxyDraft = { name, address, address_type: addressType, port, enabled: proxy?.enabled ?? true,
        include_node_name: proxy?.include_node_name ?? false, protocol: "snell", flow: "",
        config: {} }
      const saved = proxy ? await updateProxy(proxy.id, draft, node) : await createProxy(node, draft)
      if (saved.sync?.needs_sync?.length) toast.warning("已保存，节点配置仍需同步")
      else toast.success("Snell v6 已保存，请确认节点同步结果")
      onSaved(); onClose()
    } catch (error) { toast.error((error as Error).message) }
    finally { setBusy(false); onBusyChange?.(false) }
  }}>
    <DialogHeader className="shrink-0 px-6 pt-5 pb-3"><DialogTitle>{proxy ? "编辑 Snell v6" : "新增 Snell v6"}</DialogTitle></DialogHeader>
    <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-6 pb-4">
      <p className="text-sm text-muted-foreground">使用节点 sing-box。系统为每条代理生成共享 PSK。</p>
      <label className="block">节点<select aria-label="节点" className="block w-full border p-2" value={node} onChange={e => changeNode(Number(e.target.value))}>{nodes.map(n => <option key={n.id} value={n.id}>{n.name}</option>)}</select></label>
      <label className="block">名称<Input required maxLength={128} value={name} onChange={e => setName(e.target.value)} /></label>
      <ProxyAddressField selectedNode={selectedNode} addressType={addressType} address={address} changeAddressType={setAddressType} setCustomAddress={setCustomAddress} />
      <label className="block">TCP 端口<Input required type="number" min={1} max={65535} value={port} onChange={e => setPort(Number(e.target.value))} /></label>
    </div>
    <DialogFooter className="shrink-0 border-t px-6 py-3">
      {onBack && <Button type="button" variant="outline" disabled={busy} onClick={onBack}>返回</Button>}
      <Button type="button" variant="outline" disabled={busy} onClick={onClose}>取消</Button>
      <Button type="submit" disabled={busy || !node}>保存</Button>
    </DialogFooter>
  </form>
  return embedded ? form : <ProxyDialog busy={busy} onClose={onClose}>{form}</ProxyDialog>
}
