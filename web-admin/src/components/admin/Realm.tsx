import type { ReactNode, ComponentProps } from "react"
import { useCallback, useEffect, useRef, useState } from "react"
import { Pencil, Settings2, Trash2 } from "lucide-react"
import { toast } from "sonner"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Switch } from "@/components/ui/switch"
import { TableCell, TableRow } from "@/components/ui/table"
import { getComponents } from "@/lib/node-components"
import { api, type Node } from "@/lib/api"
import type { Proxy } from "@/lib/resources"
import { realmHostStatus, saveRealmRule, nodeProxyAddress, defaultProxyAddressType, type RealmHost, type RealmRule, type RealmRuleDraft } from "@/lib/realm-management"
import { trafficBytes, trafficState, type Traffic, type TrafficRule, type TrafficStatus } from "@/lib/realm-traffic"
import { ProxyDialog } from "../ProxyDialog"
import { installCommand } from "./Nodes"

const selectClass = "h-9 w-full rounded-md border bg-background px-3 text-sm"
export function useRealmProxies() {
  const [hosts, setHosts] = useState<RealmHost[]>([])
  const [rules, setRules] = useState<RealmRule[]>([])
  const [traffic, setTraffic] = useState<Traffic>({ hosts: [], rules: [] })
  const [error, setError] = useState("")
  const [loaded, setLoaded] = useState(false)
  const controller = useRef<AbortController | null>(null)
  const pending = useRef<Promise<void> | null>(null)
  const load = useCallback(() => {
    if (pending.current) return pending.current
    const signal = controller.current?.signal
    if (!signal || signal.aborted) return Promise.resolve()
    const request = (async () => {
      try {
        const results = await Promise.allSettled([
          api<{ items: RealmHost[] }>("/realm/hosts", { signal }),
          api<{ items: RealmRule[] }>("/realm/rules", { signal }),
          api<Traffic>("/realm/traffic", { signal }),
        ])
        if (signal.aborted) return
        const [h, r, t] = results
        if (h.status === "rejected") throw h.reason
        if (r.status === "rejected") throw r.reason
        if (t.status === "rejected") throw t.reason
        setHosts(h.value.items); setRules(r.value.items); setTraffic(t.value); setLoaded(true); setError("")
      } catch (e) { if (!signal.aborted) setError((e as Error).message) }
    })()
    pending.current = request
    void request.finally(() => { if (pending.current === request) pending.current = null })
    return request
  }, [])
  const reload = useCallback(async () => { await pending.current; await load() }, [load])
  useEffect(() => {
    const active = new AbortController()
    controller.current = active
    // StrictMode can mount again before the aborted request settles.
    void reload()
    const refresh = () => { if (!document.hidden && !pending.current) void load() }
    const timer = setInterval(refresh, 5000)
    document.addEventListener("visibilitychange", refresh)
    return () => { active.abort(); clearInterval(timer); document.removeEventListener("visibilitychange", refresh) }
  }, [load, reload])
  return { hosts, rules, traffic, error, loaded, reload }
}

function Field({ title, children }: { title: string; children: React.ReactNode }) {
  return <label className="flex flex-col gap-1.5 text-sm"><span className="font-medium">{title}</span>{children}</label>
}
function EntryWarning({ host }: { host?: RealmHost }) {
  if (!host) return <p className="text-sm text-muted-foreground">保存后面板管理安装器创建的 Realm 配置，并在首次应用前自动备份；节点离线或未安装时等待同步。</p>
  if (!host.online || !host.capable || host.status.installed === false || host.status.managed === false) return <p role="status" className="text-sm text-amber-600">{realmHostStatus(host)}。请先在节点安装选项中开启并保存 Realm，再执行安装命令。</p>
  return null
}
function InstallCommand({ nodeId, site, refusal }: { nodeId: number; site: string; refusal: string }) {
  const [command, setCommand] = useState("")
  const [busy, setBusy] = useState(false)
  const install = async () => {
    setBusy(true)
    setCommand("")
    try {
      const [credential, profile] = await Promise.all([api<{ token: string }>(`/nodes/${nodeId}/token`), getComponents(nodeId)])
      if (!profile.install.realm) { toast.error("请先在节点安装选项中开启并保存 Realm"); return }
      setCommand(installCommand(site, credential.token, undefined, undefined))
    }
    catch (e) { toast.error((e as Error).message) }
    finally { setBusy(false) }
  }
  return <div className="space-y-2"><Button type="button" size="sm" variant="outline" disabled={!nodeId || !!refusal || busy} title={refusal} onClick={() => void install()}>安装 / 升级命令</Button>{command && <><p className="text-xs text-muted-foreground">在所选入口节点执行，按已保存的安装选项安装或卸载组件。</p><textarea aria-label="Realm 安装命令" readOnly className="min-h-24 w-full rounded border bg-muted p-3 font-mono text-xs" value={command} /></>}</div>
}

export function RealmProxyForm({ rule, nodes, proxies, hosts, rules, ready, site, refusal, onClose, onSaved, onRefresh, onBack, embedded, onBusyChange }: {
  rule: RealmRule | null; nodes: Node[]; proxies: Proxy[]; hosts: RealmHost[]; rules: RealmRule[]; ready: boolean; site: string; refusal: string;
  onClose: () => void; onSaved: () => void; onRefresh: () => Promise<void>; onBack?: () => void; embedded?: boolean; onBusyChange?: (busy: boolean) => void;
}) {
  const initialNodeId = rule?.node_id ?? nodes[0]?.id ?? 0
  const initialNode=nodes.find((n)=>n.id===initialNodeId)
  const [draft, setDraft] = useState<RealmRuleDraft>(() => rule ? { node_id: rule.node_id, target_proxy_id: rule.target_proxy_id, name: rule.custom_name, address_type: rule.address_type, address: rule.address, listen_port: rule.listen_port, enabled: rule.enabled } : { node_id: initialNodeId, target_proxy_id: 0, name: "", address_type: defaultProxyAddressType(initialNode), address: "", listen_port: 0, enabled: true })
  const [customAddress, setCustomAddress] = useState(rule?.address_type === "domain" ? rule.address : "")
  const [targetNodeId, setTargetNodeId] = useState(rule?.target_node_id ?? 0)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState("")
  const host=hosts.find((h)=>h.node_id===draft.node_id)
  const node=nodes.find((n)=>n.id===draft.node_id)
  const savedAddress=rule?.node_id===draft.node_id && rule.address_type===draft.address_type ? rule.address : ""
  const address=draft.address_type === "domain" ? customAddress : nodeProxyAddress(node,draft.address_type) || savedAddress
  const target=proxies.find((p)=>p.id===draft.target_proxy_id)
  const changeNode=(id:number)=>{
    setDraft({...draft,node_id:id,target_proxy_id:0,address_type:defaultProxyAddressType(nodes.find((n)=>n.id===id))})
    setCustomAddress("");setTargetNodeId(0);setError("")
  }
  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!ready || saving) return
    setError("")
    if (!draft.node_id || !target || target.node_id !== targetNodeId || target.node_id === draft.node_id || (draft.enabled && !target.enabled)) { setError("请选择其他节点上有效的目标代理"); return }
    if (!draft.name.trim() || !Number.isInteger(draft.listen_port) || draft.listen_port < 1 || draft.listen_port > 65535) { setError("请填写名称和有效的监听端口"); return }
    if (rules.some((r) => r.id !== rule?.id && r.node_id === draft.node_id && r.listen_port === draft.listen_port) || proxies.some((p) => p.node_id === draft.node_id && p.port === draft.listen_port && p.enabled)) { setError("监听端口与现有代理或 Realm 规则冲突"); return }
    if (!address.trim()) { setError("请填写有效的对外连接地址"); return }
    setSaving(true);onBusyChange?.(true)
    try {
      await saveRealmRule(api, rule?.id ?? null, { ...draft, name: draft.name.trim(), address:address.trim() })
      toast.success("Realm 代理已保存，应用成功后发布订阅入口")
      onSaved(); onClose()
    } catch (e) {
      setError((e as Error).message)
      // Keep the draft visible on save failure.
      await onRefresh()
    } finally { setSaving(false);onBusyChange?.(false) }
  }
  const form = <form className="flex min-h-0 flex-col" onSubmit={submit}>
      <DialogHeader className="shrink-0 px-6 pt-6 pb-4"><DialogTitle>{rule ? `编辑 Realm 代理：${rule.name}` : "新增 Realm 代理"}</DialogTitle><DialogDescription>客户端 → 入口节点的监听端口 → 已有代理。客户端协议和授权沿用目标代理。</DialogDescription></DialogHeader>
      <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-6 pb-4">
        <Field title="代理名称"><Input aria-label="代理名称" autoFocus required maxLength={128} value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} /></Field>
        <Field title="入口节点"><select aria-label="入口节点" className={selectClass} required value={draft.node_id || ""} disabled={saving} onChange={(e) => changeNode(Number(e.target.value))}><option value="">选择已有节点</option>{nodes.map((n) => <option key={n.id} value={n.id}>{n.name}{n.online ? "" : "（离线）"}</option>)}</select></Field>
        <EntryWarning host={host} />
        <section className="space-y-2">
          <span className="text-sm font-medium">对外地址</span>
          <div className="flex gap-2" role="group" aria-label="对外地址类型">{([["ipv4","IPv4"],["ipv6","IPv6"],["domain","自定义"]] as const).map(([type,label])=><Button key={type} type="button" size="sm" variant={draft.address_type===type?"default":"outline"} aria-pressed={draft.address_type===type} disabled={saving || (type!=="domain" && !nodeProxyAddress(node,type) && !(rule?.node_id===draft.node_id && rule.address_type===type && rule.address))} onClick={()=>setDraft({...draft,address_type:type})}>{label}</Button>)}</div>
          <Field title="对外连接地址（域名 / IP，不含端口）"><Input aria-label="对外连接地址（域名 / IP，不含端口）" required readOnly={draft.address_type!=="domain"} value={address} onChange={(e)=>setCustomAddress(e.target.value)} /></Field>
          <p className="text-xs text-muted-foreground">仅用于当前代理的客户端连接地址；监听默认支持 IPv4 / IPv6。</p>
        </section>
        {host?.enabled===false && <p className="text-sm text-amber-600">节点 Realm 服务总开关已停用，规则保存后需在入口服务设置中启用。</p>}
        <Field title="监听端口"><div className="flex gap-2"><Input aria-label="监听端口" type="number" required min={1} max={65535} step={1} value={draft.listen_port || ""} onChange={(e) => setDraft({ ...draft, listen_port: Number(e.target.value) })} /><Button type="button" variant="outline" onClick={() => setDraft({ ...draft, listen_port: 20000 + Math.floor(Math.random() * 45536) })}>随机</Button></div></Field>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field title="目标节点"><select aria-label="目标节点" className={selectClass} required value={targetNodeId || ""} onChange={(e) => { setTargetNodeId(Number(e.target.value)); setDraft({ ...draft, target_proxy_id: 0 }) }}><option value="">选择目标节点</option>{nodes.filter((n) => n.id !== draft.node_id).map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}</select></Field>
          <Field title="目标代理"><select aria-label="目标代理" className={selectClass} required value={draft.target_proxy_id || ""} onChange={(e) => setDraft({ ...draft, target_proxy_id: Number(e.target.value) })}><option value="">选择已有代理</option>{proxies.filter((p) => p.node_id === targetNodeId && p.node_id !== draft.node_id && (p.enabled || p.id === rule?.target_proxy_id)).map((p) => <option key={p.id} value={p.id}>{p.name} · {p.address}:{p.port}{p.enabled ? "" : "（已停用）"}</option>)}</select></Field>
        </div>
        {draft.enabled && target && !target.enabled && <p role="alert" className="text-sm text-destructive">目标代理已停用，请选择其他代理或停用当前规则。</p>}
        <InstallCommand key={draft.node_id} nodeId={draft.node_id} site={site} refusal={refusal} />
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {!ready && <p role="status" className="text-sm text-muted-foreground">正在读取入口配置，请稍候。</p>}
      </div>
      <DialogFooter className="shrink-0 border-t px-6 py-3">{onBack && <Button type="button" variant="outline" disabled={saving} onClick={onBack}>返回选择类型</Button>}<Button type="button" variant="ghost" disabled={saving} onClick={onClose}>取消</Button><Button disabled={saving || !ready || !nodes.length}>{saving ? "保存中…" : rule ? "保存修改" : "创建代理"}</Button></DialogFooter>
    </form>
  return embedded ? form : <ProxyDialog busy={saving} onClose={onClose}>{form}</ProxyDialog>
}

export function RealmProxyRow({ rowProps, dragHandle, rule, host, nodes, busy, onEdit, onDelete, onDetail, onEnabled }: {
  rowProps?: ComponentProps<typeof TableRow>; dragHandle?: ReactNode; rule: RealmRule; host?: RealmHost; nodes: Node[]; busy: boolean;
  onEdit: () => void; onDelete: () => void; onDetail: () => void; onEnabled: (enabled: boolean) => void;
}) {
  const node = nodes.find((n) => n.id === rule.node_id)
  const targetNode = nodes.find((n) => n.id === rule.target_node_id)
  return <TableRow {...rowProps} className={`h-14 ${rowProps?.className ?? ""}`}>
    <TableCell className="px-2">{dragHandle}</TableCell>
    <TableCell className="px-3"><div className="flex items-center gap-2"><span aria-hidden="true" className={`size-2 rounded-full ${node?.online ? "bg-emerald-500" : "bg-muted-foreground/40"}`} /><div><div className="font-medium">{node?.name ?? `节点 ${rule.node_id}`}</div>{node?.group && <div className="text-xs text-muted-foreground">{node.group}</div>}</div></div></TableCell>
    <TableCell className="px-3 font-medium">{rule.name}</TableCell>
    <TableCell className="px-3"><Badge variant="outline" className="border-sky-500/35 bg-sky-500/10 font-medium text-sky-700 dark:text-sky-300">Realm 转发</Badge></TableCell>
    <TableCell className="max-w-64 px-3"><span className="block max-w-48 truncate" title={rule.address}>{rule.address ?? "—"}</span></TableCell>
    <TableCell className="tnum px-3 text-sm">{rule.listen_port}</TableCell>
    <TableCell className="px-3"><div className="max-w-56 truncate text-sm" title={`${targetNode?.name ?? ""} / ${rule.target_name}`}>→ {targetNode?.name ?? `节点 ${rule.target_node_id}`} / {rule.target_name}</div><div className="max-w-56 truncate text-xs text-muted-foreground" title={`${rule.target_address}:${rule.target_port}`}>{rule.target_address}:{rule.target_port}</div></TableCell>
    <TableCell className="px-3 text-center"><div className="flex flex-col items-center gap-1"><Switch checked={rule.enabled} disabled={busy} onCheckedChange={onEnabled} aria-label={`${rule.enabled ? "停用" : "启用"} Realm 代理 ${rule.name}`} /><span className="text-xs text-muted-foreground">{!rule.enabled ? "已停用" : !rule.target_enabled ? "目标代理已停用" : realmHostStatus(host)}</span></div></TableCell>
    <TableCell className="whitespace-nowrap px-3 text-right"><div className="flex justify-end gap-1"><Button variant="ghost" size="icon" title="流量 / 入口服务" aria-label={`流量 / 入口服务 ${rule.name}`} onClick={onDetail}><Settings2 /></Button><Button variant="ghost" size="icon" disabled={busy} aria-label={`编辑 Realm 代理 ${rule.name}`} onClick={onEdit}><Pencil /></Button><Button variant="ghost" size="icon" disabled={busy} className="text-destructive" aria-label={`删除 Realm 代理 ${rule.name}`} onClick={onDelete}><Trash2 /></Button></div></TableCell>
  </TableRow>
}

export function TrafficSummary({ items, status }: { items: TrafficRule[]; status?: TrafficStatus }) {
  const known = items.some((r) => r.last_sample)
  const sum = (field: "up" | "down" | "today" | "month") => known ? items.reduce((n, r) => n + r[field], 0) : null
  const live = items.length > 0 && items.every((r) => r.up_rate != null && r.down_rate != null)
  const upRate = status ? status.fresh ? status.up_rate : null : live ? items.reduce((n, r) => n + r.up_rate!, 0) : null
  const downRate = status ? status.fresh ? status.down_rate : null : live ? items.reduce((n, r) => n + r.down_rate!, 0) : null
  return <p className="text-xs text-muted-foreground">累计 ↑ {trafficBytes(sum("up"))} / ↓ {trafficBytes(sum("down"))} · 今日 {trafficBytes(sum("today"))} · 本月 {trafficBytes(sum("month"))} · 速率 ↑ {trafficBytes(upRate)}/s / ↓ {trafficBytes(downRate)}/s</p>
}

function RealmTrafficCard({ title, items, status }: { title: string; items: TrafficRule[]; status?: TrafficStatus }) {
  const known = items.some((item) => item.last_sample)
  const total = (field: "up" | "down" | "today" | "month") => known ? items.reduce((sum, item) => sum + item[field], 0) : null
  const live = items.length > 0 && items.every((item) => item.up_rate != null && item.down_rate != null)
  const rate = (field: "up_rate" | "down_rate") => status ? status.fresh ? status[field] : null : live ? items.reduce((sum, item) => sum + item[field]!, 0) : null
  return <section className="rounded-xl border bg-card p-4"><div className="mb-4 flex items-center justify-between gap-2"><h3 className="truncate text-sm font-medium" title={title}>{title}</h3>{status && <span className="shrink-0 text-xs text-muted-foreground">{trafficState(status)}</span>}</div><dl className="grid grid-cols-2 gap-x-4 gap-y-4 sm:grid-cols-4">{([
    ["累计上传", total("up")], ["累计下载", total("down")], ["今日", total("today")], ["本月", total("month")],
  ] as const).map(([label, value]) => <div key={label}><dt className="mb-1 whitespace-nowrap text-xs text-muted-foreground">{label}</dt><dd className="tnum whitespace-nowrap text-sm font-semibold">{trafficBytes(value)}</dd></div>)}</dl><div className="mt-4 flex flex-wrap gap-x-5 gap-y-1 border-t pt-3 text-xs text-muted-foreground"><span className="whitespace-nowrap">上传 ↑ {trafficBytes(rate("up_rate"))}/s</span><span className="whitespace-nowrap">下载 ↓ {trafficBytes(rate("down_rate"))}/s</span></div></section>
}

export function RealmEntryDialog({ host, rule, traffic, onClose }: {
  host: RealmHost; rule?: RealmRule; traffic: Traffic; onClose: () => void;
}) {
  const stats = traffic.hosts.find((item) => item.node_id === host.node_id)?.status
  const version = host.status.version?.match(/(?:Realm\s+)?\d+\.\d+\.\d+/)?.[0] ?? "版本未知"
  const reachability = host.status.target_reachability === "reachable" ? "目标可连接" : host.status.target_reachability === "unreachable" ? "目标不可连接" : host.status.target_reachability === "partial" ? "目标部分可连接" : "目标未检测"
  return <Dialog open onOpenChange={(open) => { if (!open) onClose() }}><DialogContent className="flex max-h-[90svh] flex-col gap-0 overflow-hidden p-0 sm:max-w-xl">
    <DialogHeader className="shrink-0 px-5 pt-5 pb-4 text-left"><DialogTitle className="truncate pr-6" title={host.name}>{host.name} · Realm</DialogTitle><DialogDescription>入口状态与流量</DialogDescription></DialogHeader>
    <div className="min-h-0 space-y-4 overflow-y-auto px-5 pb-5">
      <section className="space-y-3 rounded-xl bg-muted/40 p-4"><div className="flex flex-wrap items-center gap-2"><Badge variant="outline" className={host.status.running ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" : ""}>{realmHostStatus(host)}</Badge><span className="text-xs text-muted-foreground">{version}</span><span className="ml-auto text-xs text-muted-foreground">同步 {host.applied_revision} / {host.desired_revision}</span></div><p className="text-xs text-muted-foreground">{reachability} · 不代表完整代理链路可用</p><EntryWarning host={host} />{host.last_error && <p role="alert" className="break-words text-sm text-destructive">{host.last_error}</p>}</section>
      {rule && <RealmTrafficCard title={`${rule.name} · 规则流量`} items={traffic.rules.filter((item) => item.key === rule.traffic_key)} />}
      <RealmTrafficCard title="节点 Realm 流量" items={traffic.rules.filter((item) => item.node_id === host.node_id)} status={stats} />
      {(!host.online || !host.traffic_capable || (stats && !stats.fresh)) && <p role="status" className="text-xs text-amber-700 dark:text-amber-300">{!host.online ? "节点离线，显示最近记录" : !host.traffic_capable ? "升级 Agent 后可监控流量" : trafficState(stats)}</p>}
      {!!((stats?.gaps ?? 0) || (stats?.discarded_records ?? 0)) && <p role="status" className="text-xs text-amber-700 dark:text-amber-300">采集缺口：{stats?.gaps ?? 0} · 无法归属记录：{stats?.discarded_records ?? 0}</p>}
      <p className="text-xs text-muted-foreground">最近采集：{stats?.last_sample ? new Date(stats.last_sample * 1000).toLocaleString() : "—"}</p>
      <details className="rounded-lg border px-3 py-2 text-xs text-muted-foreground"><summary className="cursor-pointer py-1">更多信息</summary><div className="space-y-2 pt-2"><p className="break-words">版本：{host.status.version ?? "未知"}</p><p>已应用 {host.applied_revision} / 期望 {host.desired_revision}</p><p>采集缺口：{stats?.gaps ?? "—"} · 无法归属记录：{stats?.discarded_records ?? 0}</p><p>仅统计入口客户端侧 TCP，包含协议开销和重传，排除 loopback；与整机网卡分开，不用于用户扣费。今日 / 本月按 UTC，缺失时段不补算。</p></div></details>
    </div><DialogFooter className="shrink-0 border-t px-5 py-3"><Button variant="outline" onClick={onClose}>关闭</Button></DialogFooter>
  </DialogContent></Dialog>
}
