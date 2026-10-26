import { useEffect, useId, useState } from "react"
import { toast } from "sonner"
import { api, type Node } from "@/lib/api"
import { createProxy, updateProxy, type Proxy, type AddressType } from "@/lib/resources"
import { defaultProxyAddressType, nodeProxyAddress } from "@/lib/realm-management"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { DialogHeader, DialogTitle, DialogDescription } from "@/components/ui/dialog"
import { ProxyDialog } from "@/components/ProxyDialog"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"

type Host = { node_id: number; revision: number; applied_revision: number; capable: boolean; online: boolean; last_error: string; traffic_error?: string; status: { running?: boolean; installed?: boolean } }
export function MieruForm({ proxy, nodes, embedded, onClose, onSaved, onBack, onBusyChange }: { proxy: Proxy | null; nodes: Node[]; embedded?: boolean; onClose: () => void; onSaved: () => void; onBack?: () => void; onBusyChange?: (busy: boolean) => void }) {
  const [nodeId, setNodeId] = useState(proxy?.node_id ?? nodes[0]?.id ?? 0)
  const [name, setName] = useState(proxy?.name ?? "mieru")
  const [addressType, setAddressType] = useState<AddressType>(proxy?.address_type ?? defaultProxyAddressType(nodes[0]))
  const [customAddress, setCustomAddress] = useState(proxy?.address_type === "domain" ? proxy.address : "")
  const selectedNode = nodes.find((node) => node.id === nodeId)
  const address = addressType === "domain" ? customAddress : nodeProxyAddress(selectedNode, addressType) || (proxy?.node_id === nodeId && proxy.address_type === addressType ? proxy.address : "")
  function changeNode(value: string) {
    const nextId = Number(value)
    const nextNode = nodes.find((node) => node.id === nextId)
    setNodeId(nextId)
    if (addressType !== "domain" && !nodeProxyAddress(nextNode, addressType)) setAddressType(defaultProxyAddressType(nextNode))
  }
  const [port, setPort] = useState(proxy?.port ?? 24061)
  const fieldId = useId()
  const editing = proxy !== null
  const [busy, setBusy] = useState(false)
  const [hosts, setHosts] = useState<Host[]>([])
  const [error, setError] = useState("")
  useEffect(() => {
    if (!editing) return
    const controller = new AbortController()
    const load = () => api<{ items: Host[] }>("/mieru/hosts", { signal: controller.signal }).then((v) => { setHosts(v.items); setError("") }).catch((e: Error) => { if (!controller.signal.aborted) setError(e.message) })
    void load(); const timer = setInterval(() => void load(), 5000)
    return () => { controller.abort(); clearInterval(timer) }
  }, [editing])
  const host = hosts.find((h) => h.node_id === nodeId)
  async function save(e: React.FormEvent) {
    e.preventDefault(); setBusy(true); onBusyChange?.(true)
    try {
      const address_type = addressType
      const draft = { name, address, address_type, port, enabled: proxy?.enabled ?? true, include_node_name: proxy?.include_node_name ?? true, protocol: "mieru" as const, flow: "" as const, config: { transport: "tcp" as const } }
      if (proxy) await updateProxy(proxy.id, draft, nodeId); else await createProxy(nodeId, draft)
      toast.success("代理已保存，等待同步")
      onSaved(); onClose()
    } catch (e) { toast.error((e as Error).message) } finally { setBusy(false); onBusyChange?.(false) }
  }
  const statusError = error || host?.last_error || host?.traffic_error
  const statusText = error || (host
    ? !host.online ? "节点离线，等待重连"
      : !host.capable ? "需要升级 Agent"
        : host.status.installed === false ? "未安装 mieru"
          : statusError || (host.revision !== host.applied_revision ? "配置同步中" : host.status.running ? "运行中" : "已停止")
    : "正在读取节点状态")
  const form = (
    <form className="flex min-h-0 flex-1 flex-col" onSubmit={(e) => void save(e)}>
      <DialogHeader className="shrink-0 px-6 pt-6 pb-5 text-left">
        <DialogTitle className="whitespace-nowrap pr-6">{editing ? "编辑 mieru 代理" : "新增 mieru 代理"}</DialogTitle>
        <DialogDescription className="whitespace-nowrap text-xs">TCP · 每节点一个代理</DialogDescription>
      </DialogHeader>
      <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 pb-6">
        <div className="space-y-2">
          <Label htmlFor={`${fieldId}-name`} className="whitespace-nowrap">代理名称</Label>
          <Input id={`${fieldId}-name`} autoFocus={!editing} required maxLength={128} disabled={busy} value={name} placeholder="例如：香港 mieru" onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="space-y-2">
          <Label htmlFor={`${fieldId}-node`} className="whitespace-nowrap">所属节点</Label>
          <Select value={String(nodeId || "")} disabled={busy || !nodes.length} onValueChange={changeNode}>
            <SelectTrigger id={`${fieldId}-node`} className="w-full min-w-0"><SelectValue placeholder="选择节点" /></SelectTrigger>
            <SelectContent position="popper">
              {nodes.map((node) => <SelectItem key={node.id} value={String(node.id)}><span className="truncate" title={node.name}>{node.name}</span></SelectItem>)}
            </SelectContent>
          </Select>
        </div>
        <div className="grid min-w-0 grid-cols-1 gap-5 sm:grid-cols-[minmax(0,1fr)_7.5rem] sm:items-end sm:gap-3">
          <div className="min-w-0 space-y-2">
            <Label htmlFor={`${fieldId}-address`} className="whitespace-nowrap">连接地址</Label>
            <div className="flex gap-1" role="group" aria-label="连接地址类型">
              {([["ipv4", "IPv4"], ["ipv6", "IPv6"], ["domain", "自定义"]] as const).map(([type, label]) => <Button key={type} type="button" size="sm" variant={addressType === type ? "default" : "outline"} aria-pressed={addressType === type} disabled={busy || (type !== "domain" && !nodeProxyAddress(selectedNode, type) && !(proxy?.node_id === nodeId && proxy.address_type === type && proxy.address))} onClick={() => setAddressType(type)}>{label}</Button>)}
            </div>
            <Input id={`${fieldId}-address`} required disabled={busy} readOnly={addressType !== "domain"} value={address} placeholder={addressType === "domain" ? "域名或 IP 地址" : "节点尚未上报此地址"} onChange={(e) => setCustomAddress(e.target.value.trim())} />
          </div>
          <div className="min-w-0 space-y-2">
            <Label htmlFor={`${fieldId}-port`} className="whitespace-nowrap">端口</Label>
            <Input id={`${fieldId}-port`} required disabled={busy} type="number" min={1025} max={65535} value={port} onChange={(e) => setPort(Number(e.target.value))} />
          </div>
        </div>
        {editing && <div className={`flex min-w-0 items-center gap-2 rounded-md bg-muted/40 px-3 py-2.5 text-xs ${statusError ? "text-destructive" : "text-muted-foreground"}`}>
          <span aria-hidden="true" className={`size-1.5 shrink-0 rounded-full ${statusError ? "bg-destructive" : host?.status.running ? "bg-emerald-500" : "bg-muted-foreground/50"}`} />
          <p role="status" className="min-w-0 truncate" title={statusText}>{statusText}</p>
        </div>}
      </div>
      <div className="flex shrink-0 items-center justify-between gap-3 border-t bg-muted/20 px-6 py-4">
        <div>{onBack && <Button type="button" variant="ghost" disabled={busy} onClick={onBack}>返回</Button>}</div>
        <div className="flex shrink-0 items-center gap-2">
          <Button type="button" variant="outline" disabled={busy} onClick={onClose}>取消</Button>
          <Button disabled={busy || !nodeId}>{busy ? "保存中…" : "保存"}</Button>
        </div>
      </div>
    </form>
  )
  return embedded ? form : <ProxyDialog busy={busy} onClose={onClose}>{form}</ProxyDialog>
}

export function MieruCredential({ userId, onClose }: { userId: number; onClose: () => void }) {
  const [credential, setCredential] = useState<{ username: string; password: string } | null>(null)
  const [error, setError] = useState("")
  const [busy, setBusy] = useState(false)
  const [show, setShow] = useState(false)
  useEffect(() => { const controller = new AbortController(); void api<{ username: string; password: string }>(`/users/${userId}/mieru-credential`, { signal: controller.signal }).then(setCredential).catch((e: Error) => { if (!controller.signal.aborted) setError(e.message) }); return () => controller.abort() }, [userId])
  async function reset() {
    if (!confirm("重置 mieru 连接密码？旧客户端配置需要重新导入，节点将在同步后使用新密码。")) return
    setBusy(true)
    try { setCredential(await api(`/users/${userId}/mieru-credential`, { method: "POST" })); setError(""); toast.success("已重置，等待关联节点同步") } catch (e) { setError((e as Error).message) } finally { setBusy(false) }
  }
  return <ProxyDialog busy={busy} onClose={onClose}><DialogHeader className="p-6 pb-0"><DialogTitle>mieru 连接凭据</DialogTitle><DialogDescription>独立于面板登录密码；重置会更新该用户所有 mieru 节点。</DialogDescription></DialogHeader><div className="space-y-4 p-6">{error && <p role="alert" className="text-destructive">{error}</p>}<Label>连接用户名<Input readOnly value={credential?.username ?? ""} /></Label><Label>连接密码<Input readOnly type={show ? "text" : "password"} value={credential?.password ?? ""} /></Label><div className="flex justify-end gap-2"><Button variant="ghost" onClick={() => setShow(!show)}>{show ? "隐藏密码" : "显示密码"}</Button><Button variant="outline" disabled={!credential || busy} onClick={() => { if (credential) void navigator.clipboard.writeText(JSON.stringify(credential)).then(() => toast.success("已复制凭据"), () => setError("复制失败")) }}>复制凭据</Button><Button variant="destructive" disabled={busy || !credential} onClick={() => void reset()}>重置密码</Button></div></div></ProxyDialog>
}
