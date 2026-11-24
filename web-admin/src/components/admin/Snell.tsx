import { ProxyAddressField, defaultAddressType, nodeAddress } from "../ProxyAddressField"
import { useState } from "react"
import { toast } from "sonner"
import type { Node } from "@/lib/api"
import { createProxy, updateProxy, type Proxy, type ProxyDraft, type AddressType } from "@/lib/resources"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ProxyDialog } from "@/components/ProxyDialog"
import { ProxyNamePrefix } from "@/components/ProxyNamePrefix"
import { DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"

export function SnellForm({ proxy, nodes, onClose, onSaved, onBack, embedded, onBusyChange }: {
  proxy: Proxy | null; nodes: Node[]; onClose: () => void; onSaved: () => void;
  onBack?: () => void; embedded?: boolean; onBusyChange?: (busy: boolean) => void
}) {
  const [node, setNode] = useState(proxy?.node_id ?? nodes[0]?.id ?? 0)
  const [name, setName] = useState(proxy?.name ?? "Snell")
  const [includeNodeName, setIncludeNodeName] = useState(proxy?.include_node_name ?? true)
  const config = proxy?.config?.snell
  const [version, setVersion] = useState<5 | 6>(config?.version === 5 ? 5 : 6)
  const [psk, setPsk] = useState(config?.psk_override ?? "")
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
        include_node_name: includeNodeName, protocol: "snell", flow: "",
        config: { snell: { version, psk_override: psk.trim() ? psk : null } } }
      const saved = proxy ? await updateProxy(proxy.id, draft, node) : await createProxy(node, draft)
      if (saved.sync?.needs_sync?.length) toast.warning("已保存，节点配置仍需同步")
      else toast.success("Snell 已保存，请确认节点同步结果")
      onSaved(); onClose()
    } catch (error) { toast.error((error as Error).message) }
    finally { setBusy(false); onBusyChange?.(false) }
  }}>
    <DialogHeader className="shrink-0 px-6 pt-5 pb-3"><DialogTitle>{proxy ? "编辑 Snell" : "新增 Snell"}</DialogTitle></DialogHeader>
    <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-6 pb-4">
      <label className="block">节点<select aria-label="节点" className="block w-full border p-2" value={node} onChange={e => changeNode(Number(e.target.value))}>{nodes.map(n => <option key={n.id} value={n.id}>{n.name}</option>)}</select></label>
      <label className="block">名称<Input required maxLength={128} value={name} onChange={e => setName(e.target.value)} /></label>
      <ProxyNamePrefix name={name} node={selectedNode} includeNodeName={includeNodeName} setIncludeNodeName={setIncludeNodeName} />
      <label className="block">协议版本<select className="block w-full border p-2" value={version} onChange={e => setVersion(e.target.value === "5" ? 5 : 6)}>
        <option value={5}>Snell v5</option>
        <option value={6}>Snell v6</option>
      </select></label>
      <label className="block">Snell PSK
        <Input type="password" autoComplete="new-password" value={psk} onChange={e => setPsk(e.target.value)} />
        <span className="text-sm text-muted-foreground">留空时使用节点的共享 PSK。</span>
      </label>
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
