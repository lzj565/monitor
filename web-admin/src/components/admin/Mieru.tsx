import { useEffect, useState } from "react"
import { toast } from "sonner"
import { api, type Node } from "@/lib/api"
import { createProxy, updateProxy, type Proxy, type AddressType } from "@/lib/resources"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { DialogHeader, DialogTitle, DialogDescription } from "@/components/ui/dialog"
import { ProxyDialog } from "@/components/ProxyDialog"
import { Switch } from "@/components/ui/switch"

type Host = { node_id: number; revision: number; applied_revision: number; capable: boolean; online: boolean; last_error: string; traffic_error?: string; status: { running?: boolean; installed?: boolean } }
export function MieruForm({ proxy, nodes, embedded, onClose, onSaved, onBack, onBusyChange }: { proxy: Proxy | null; nodes: Node[]; embedded?: boolean; onClose: () => void; onSaved: () => void; onBack?: () => void; onBusyChange?: (busy: boolean) => void }) {
  const [nodeId, setNodeId] = useState(proxy?.node_id ?? nodes[0]?.id ?? 0)
  const [name, setName] = useState(proxy?.name ?? "mieru")
  const [address, setAddress] = useState(proxy?.address ?? nodes[0]?.addresses?.[0]?.address ?? "")
  const [port, setPort] = useState(proxy?.port ?? 24061)
  const [enabled, setEnabled] = useState(proxy?.enabled ?? true)
  const [busy, setBusy] = useState(false)
  const [hosts, setHosts] = useState<Host[]>([])
  const [error, setError] = useState("")
  useEffect(() => {
    const controller = new AbortController()
    const load = () => api<{ items: Host[] }>("/mieru/hosts", { signal: controller.signal }).then((v) => { setHosts(v.items); setError("") }).catch((e: Error) => { if (!controller.signal.aborted) setError(e.message) })
    void load(); const timer = setInterval(() => void load(), 5000)
    return () => { controller.abort(); clearInterval(timer) }
  }, [])
  const host = hosts.find((h) => h.node_id === nodeId)
  async function save(e: React.FormEvent) {
    e.preventDefault(); setBusy(true); onBusyChange?.(true)
    try {
      const address_type: AddressType = address.includes(":") ? "ipv6" : /^\d+\.\d+\.\d+\.\d+$/.test(address) ? "ipv4" : "domain"
      const draft = { name, address, address_type, port, enabled, include_node_name: proxy?.include_node_name ?? true, protocol: "mieru" as const, flow: "" as const, config: { transport: "tcp" as const } }
      if (proxy) await updateProxy(proxy.id, draft, nodeId); else await createProxy(nodeId, draft)
      toast.success("已保存 mieru，等待节点同步；没有有效授权用户时服务保持停止")
      onSaved(); onClose()
    } catch (e) { toast.error((e as Error).message) } finally { setBusy(false); onBusyChange?.(false) }
  }
  const form = <><DialogHeader className="px-6 pt-6"><DialogTitle>{proxy ? "编辑 mieru 代理" : "新增 mieru 代理"}</DialogTitle><DialogDescription>每节点最多一条 mieru 代理，使用单端口 TCP。请先在节点安装选项中安装 mieru。</DialogDescription></DialogHeader><form className="space-y-4 p-6" onSubmit={(e) => void save(e)}>
    <Label>节点<select className="mt-2 block w-full rounded border bg-background p-2" value={nodeId} disabled={busy} onChange={(e) => setNodeId(Number(e.target.value))}>{nodes.map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}</select></Label>
    <Label>名称<Input className="mt-2" required maxLength={128} value={name} onChange={(e) => setName(e.target.value)} /></Label>
    <Label>对外地址<Input className="mt-2" required value={address} onChange={(e) => setAddress(e.target.value.trim())} /></Label>
    <Label>TCP 端口<Input className="mt-2" required type="number" min={1025} max={65535} value={port} onChange={(e) => setPort(Number(e.target.value))} /></Label>
    <Label>启用<Switch checked={enabled} onCheckedChange={setEnabled} disabled={busy} /></Label>
    <p role="status" className="text-sm text-muted-foreground">{error || (host ? !host.online ? "节点离线，等待重连" : !host.capable ? "需要升级 Agent" : host.status.installed === false ? "未安装 mieru，请运行节点安装命令" : host.last_error || host.traffic_error || `期望版本 ${host.revision} / 已应用 ${host.applied_revision} · ${host.status.running ? "运行中" : "已停止"}` : "保存后开始同步")}</p>
    <div className="flex justify-end gap-2">{onBack && <Button type="button" variant="ghost" disabled={busy} onClick={onBack}>返回</Button>}<Button type="button" variant="outline" disabled={busy} onClick={onClose}>取消</Button><Button disabled={busy}>保存</Button></div>
  </form></>
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
