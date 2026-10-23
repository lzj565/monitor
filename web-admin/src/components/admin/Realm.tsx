import { useCallback, useEffect, useRef, useState } from "react"
import { Pencil, Settings2, Trash2 } from "lucide-react"
import { toast } from "sonner"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Switch } from "@/components/ui/switch"
import { TableCell, TableRow } from "@/components/ui/table"
import { api, type Node } from "@/lib/api"
import type { Proxy } from "@/lib/resources"
import { realmHostStatus, saveRealmRule, type RealmHost, type RealmHostDraft, type RealmRule, type RealmRuleDraft } from "@/lib/realm-management"
import { trafficBytes, trafficState, type Traffic, type TrafficRule, type TrafficStatus } from "@/lib/realm-traffic"
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

function nodeAddress(node?: Node) {
  return node?.addresses?.find((a) => a.address && !a.address.includes(":"))?.address ?? node?.addresses?.find((a) => a.address)?.address ?? ""
}
function Field({ title, children }: { title: string; children: React.ReactNode }) {
  return <label className="flex flex-col gap-1.5 text-sm"><span className="font-medium">{title}</span>{children}</label>
}
function EntryWarning({ host }: { host?: RealmHost }) {
  if (!host) return <p className="text-sm text-muted-foreground">首次使用该节点时确认接管 Realm 配置；节点离线或尚未安装时，保存后等待安装并同步。</p>
  if (!host.online || !host.capable || host.status.installed === false || host.status.managed === false) return <p role="status" className="text-sm text-amber-600">{realmHostStatus(host)}。请使用下方安装命令安装 Realm，并在需要时升级 Agent；保存规则后等待同步。</p>
  return null
}
function InstallCommand({ nodeId, site, refusal }: { nodeId: number; site: string; refusal: string }) {
  const [command, setCommand] = useState("")
  const [busy, setBusy] = useState(false)
  const install = async () => {
    setBusy(true)
    try { const { token } = await api<{ token: string }>(`/nodes/${nodeId}/token`); setCommand(installCommand(site, token, undefined, undefined, true)) }
    catch (e) { toast.error((e as Error).message) }
    finally { setBusy(false) }
  }
  return <div className="space-y-2"><Button type="button" size="sm" variant="outline" disabled={!nodeId || !!refusal || busy} title={refusal} onClick={() => void install()}>安装 / 升级命令</Button>{command && <><p className="text-xs text-muted-foreground">在所选入口节点执行。安装 Realm 本身不会赋予旧 Agent 管理能力，旧 Agent 需一起升级。</p><textarea aria-label="Realm 安装命令" readOnly className="min-h-24 w-full rounded border bg-muted p-3 font-mono text-xs" value={command} /></>}</div>
}

export function RealmProxyForm({ rule, nodes, proxies, hosts, rules, ready, site, refusal, onClose, onSaved, onRefresh, onTypeChange }: {
  rule: RealmRule | null; nodes: Node[]; proxies: Proxy[]; hosts: RealmHost[]; rules: RealmRule[]; ready: boolean; site: string; refusal: string;
  onClose: () => void; onSaved: () => void; onRefresh: () => Promise<void>; onTypeChange?: (type: "vless" | "realm") => void;
}) {
  const initialNodeId = rule?.node_id ?? nodes[0]?.id ?? 0
  const initialHost = hosts.find((h) => h.node_id === initialNodeId)
  const [draft, setDraft] = useState<RealmRuleDraft>(() => rule ? { node_id: rule.node_id, target_proxy_id: rule.target_proxy_id, name: rule.name, listen_host: rule.listen_host, listen_port: rule.listen_port, enabled: rule.enabled } : { node_id: initialNodeId, target_proxy_id: 0, name: "", listen_host: "0.0.0.0", listen_port: 0, enabled: true })
  const [address, setAddress] = useState(initialHost?.public_address ?? nodeAddress(nodes.find((n) => n.id === initialNodeId)))
  const [serviceEnabled, setServiceEnabled] = useState(initialHost?.enabled ?? true)
  const [confirmed, setConfirmed] = useState(false)
  const [targetNodeId, setTargetNodeId] = useState(rule?.target_node_id ?? 0)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState("")
  const host = hosts.find((h) => h.node_id === draft.node_id)
  const sharedChanged = !!host && (host.public_address !== address.trim() || host.enabled !== serviceEnabled)
  const needsConfirmation = !host || sharedChanged
  const target = proxies.find((p) => p.id === draft.target_proxy_id)
  const changeNode = (id: number) => {
    const existing = hosts.find((h) => h.node_id === id)
    setDraft({ ...draft, node_id: id, target_proxy_id: 0 }); setTargetNodeId(0)
    setAddress(existing?.public_address ?? nodeAddress(nodes.find((n) => n.id === id)))
    setServiceEnabled(existing?.enabled ?? true); setConfirmed(false); setError("")
  }
  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!ready || saving) return
    setError("")
    if (!draft.node_id || !target || target.node_id !== targetNodeId || target.node_id === draft.node_id || (draft.enabled && !target.enabled)) { setError("请选择其他节点上有效的目标代理"); return }
    if (!draft.name.trim() || !Number.isInteger(draft.listen_port) || draft.listen_port < 1 || draft.listen_port > 65535) { setError("请填写名称和有效的监听端口"); return }
    if (rules.some((r) => r.id !== rule?.id && r.node_id === draft.node_id && r.listen_port === draft.listen_port) || proxies.some((p) => p.node_id === draft.node_id && p.port === draft.listen_port && p.enabled)) { setError("监听端口与现有代理或 Realm 规则冲突"); return }
    if (needsConfirmation && !confirmed) { setError("请确认节点配置接管或共享设置修改"); return }
    setSaving(true)
    try {
      await saveRealmRule(api, rule?.id ?? null, { ...draft, name: draft.name.trim() }, { node_id: draft.node_id, public_address: address.trim(), enabled: serviceEnabled, takeover: true }, host)
      toast.success("Realm 代理已保存，应用成功后发布订阅入口")
      onSaved(); onClose()
    } catch (e) {
      setError((e as Error).message)
      // The entry settings may already have been saved; retain the user's rule
      // and refresh that shared state rather than deleting it on a partial failure.
      await onRefresh()
    } finally { setSaving(false) }
  }
  return <Dialog open onOpenChange={(open) => { if (!open && !saving) onClose() }}><DialogContent className="flex max-h-[90svh] flex-col overflow-hidden p-0 sm:max-w-2xl">
    <form className="flex min-h-0 flex-col" onSubmit={submit}>
      <DialogHeader className="shrink-0 px-6 pt-6 pb-4"><DialogTitle>{rule ? `编辑 Realm 代理：${rule.name}` : "新增代理"}</DialogTitle><DialogDescription>客户端 → 入口节点的监听端口 → 已有代理。客户端协议和授权沿用目标代理。</DialogDescription></DialogHeader>
      <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-6 pb-4">
        <Field title="代理类型"><select aria-label="代理类型" className={selectClass} value="realm" disabled={!!rule || saving} onChange={(e) => onTypeChange?.(e.target.value as "vless" | "realm")}><option value="vless">VLESS Reality</option><option value="realm">Realm 转发</option></select></Field>
        <Field title="代理名称"><Input aria-label="代理名称" autoFocus required maxLength={128} value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} /></Field>
        <Field title="入口节点"><select aria-label="入口节点" className={selectClass} required value={draft.node_id || ""} disabled={saving} onChange={(e) => changeNode(Number(e.target.value))}><option value="">选择已有节点</option>{nodes.map((n) => <option key={n.id} value={n.id}>{n.name}{n.online ? "" : "（离线）"}</option>)}</select></Field>
        <EntryWarning host={host} />
        <Field title="对外连接地址（域名 / IP，不含端口）"><Input aria-label="对外连接地址（域名 / IP，不含端口）" required value={address} onChange={(e) => { setAddress(e.target.value); setConfirmed(false) }} /></Field>
        <p className="text-xs text-muted-foreground">对外地址和 Realm 服务开关是入口节点共享设置，修改会影响此节点全部 Realm 规则。</p>
        <label className="flex items-start gap-2 text-sm"><input type="checkbox" checked={serviceEnabled} onChange={(e) => { setServiceEnabled(e.target.checked); setConfirmed(false) }} />启用入口节点的 Realm 服务，有有效规则后自动启动</label>
        {!serviceEnabled && <p className="text-sm text-amber-600">入口服务已停用；即使启用当前规则，也不会开始转发。</p>}
        {needsConfirmation && <label className="flex items-start gap-2 text-sm"><input type="checkbox" required checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} />{host ? "确认修改此节点共享设置，影响全部 Realm 规则。" : "确认由面板接管此节点上安装器管理的 Realm 配置，首次应用前备份原配置。"}</label>}
        <div className="grid gap-3 sm:grid-cols-2">
          <Field title="监听地址"><select aria-label="监听地址" className={selectClass} value={draft.listen_host} onChange={(e) => setDraft({ ...draft, listen_host: e.target.value })}><option value="0.0.0.0">IPv4 · 0.0.0.0</option><option value="::">IPv6 · ::</option></select></Field>
          <Field title="监听端口"><div className="flex gap-2"><Input aria-label="监听端口" type="number" required min={1} max={65535} step={1} value={draft.listen_port || ""} onChange={(e) => setDraft({ ...draft, listen_port: Number(e.target.value) })} /><Button type="button" variant="outline" onClick={() => setDraft({ ...draft, listen_port: 20000 + Math.floor(Math.random() * 45536) })}>随机</Button></div></Field>
          <Field title="目标节点"><select aria-label="目标节点" className={selectClass} required value={targetNodeId || ""} onChange={(e) => { setTargetNodeId(Number(e.target.value)); setDraft({ ...draft, target_proxy_id: 0 }) }}><option value="">选择目标节点</option>{nodes.filter((n) => n.id !== draft.node_id).map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}</select></Field>
          <Field title="目标代理"><select aria-label="目标代理" className={selectClass} required value={draft.target_proxy_id || ""} onChange={(e) => setDraft({ ...draft, target_proxy_id: Number(e.target.value) })}><option value="">选择已有代理</option>{proxies.filter((p) => p.node_id === targetNodeId && p.node_id !== draft.node_id && (p.enabled || p.id === rule?.target_proxy_id)).map((p) => <option key={p.id} value={p.id}>{p.name} · {p.address}:{p.port}{p.enabled ? "" : "（已停用）"}</option>)}</select></Field>
        </div>
        <label className="flex gap-2 text-sm"><input type="checkbox" checked={draft.enabled} onChange={(e) => setDraft({ ...draft, enabled: e.target.checked })} />启用当前转发规则</label>
        {draft.enabled && target && !target.enabled && <p role="alert" className="text-sm text-destructive">目标代理已停用，请选择其他代理或停用当前规则。</p>}
        <InstallCommand key={draft.node_id} nodeId={draft.node_id} site={site} refusal={refusal} />
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {!ready && <p role="status" className="text-sm text-muted-foreground">正在读取入口配置，请稍候。</p>}
      </div>
      <DialogFooter className="shrink-0 border-t px-6 py-3"><Button type="button" variant="ghost" disabled={saving} onClick={onClose}>取消</Button><Button disabled={saving || !ready || !nodes.length || (needsConfirmation && !confirmed)}>{saving ? "保存中…" : rule ? "保存修改" : "创建代理"}</Button></DialogFooter>
    </form>
  </DialogContent></Dialog>
}

export function RealmProxyRow({ rule, host, nodes, busy, onEdit, onDelete, onDetail, onEnabled }: {
  rule: RealmRule; host?: RealmHost; nodes: Node[]; busy: boolean;
  onEdit: () => void; onDelete: () => void; onDetail: () => void; onEnabled: (enabled: boolean) => void;
}) {
  const node = nodes.find((n) => n.id === rule.node_id)
  const targetNode = nodes.find((n) => n.id === rule.target_node_id)
  return <TableRow className="h-14">
    <TableCell className="px-2" />
    <TableCell className="px-3"><div className="flex items-center gap-2"><span aria-hidden="true" className={`size-2 rounded-full ${node?.online ? "bg-emerald-500" : "bg-muted-foreground/40"}`} /><div><div className="font-medium">{node?.name ?? `节点 ${rule.node_id}`}</div>{node?.group && <div className="text-xs text-muted-foreground">{node.group}</div>}</div></div></TableCell>
    <TableCell className="px-3 font-medium">{rule.name}</TableCell>
    <TableCell className="px-3"><Badge variant="outline">Realm 转发</Badge></TableCell>
    <TableCell className="max-w-64 px-3"><span className="block max-w-48 truncate" title={host?.public_address}>{host?.public_address ?? "—"}</span></TableCell>
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

export function RealmEntryDialog({ host, rule, rules, traffic, site, refusal, onClose, onSaved }: {
  host: RealmHost; rule?: RealmRule; rules: RealmRule[]; traffic: Traffic; site: string; refusal: string; onClose: () => void; onSaved: () => Promise<void>;
}) {
  const [address, setAddress] = useState(host.public_address)
  const [enabled, setEnabled] = useState(host.enabled)
  const [confirmed, setConfirmed] = useState(false)
  const [busy, setBusy] = useState(false)
  const [preview, setPreview] = useState("")
  const stats = traffic.hosts.find((h) => h.node_id === host.node_id)?.status
  const changed = host.public_address !== address.trim() || host.enabled !== enabled
  const perform = async (work: () => Promise<unknown>, message: string) => {
    setBusy(true)
    try { await work(); toast.success(message); await onSaved() } catch (e) { toast.error((e as Error).message) }
    finally { setBusy(false) }
  }
  const operation = (action: string) => api(`/realm/hosts/${host.node_id}/operations`, { method: "POST", body: JSON.stringify({ action }) })
  return <Dialog open onOpenChange={(open) => { if (!open && !busy) onClose() }}><DialogContent className="max-h-[90svh] overflow-y-auto sm:max-w-2xl"><DialogHeader><DialogTitle>{host.name} · Realm 入口服务</DialogTitle><DialogDescription>入口节点共享服务设置。调整地址、服务启停或重启会影响此节点全部 Realm 规则。</DialogDescription></DialogHeader>
    <div className="space-y-4">
      <p className="text-sm">{realmHostStatus(host)} · 期望 {host.desired_revision} / 已应用 {host.applied_revision} · {host.status.version ?? "版本未知"}</p>
      <EntryWarning host={host} />
      {host.last_error && <p role="alert" className="break-all text-sm text-destructive">{host.last_error}</p>}
      <p className="text-xs text-muted-foreground">目标可达：{host.status.target_reachability === "reachable" ? "已检测目标可连接" : host.status.target_reachability === "unreachable" ? "检测目标不可连接" : host.status.target_reachability === "partial" ? "部分可连接 / 部分未检测" : "未检测"}。服务运行不代表完整代理链路可用。</p>
      {rule && <section className="space-y-2"><h3 className="text-sm font-medium">{rule.name} · 规则流量</h3><TrafficSummary items={traffic.rules.filter((r) => r.key === rule.traffic_key)} /></section>}
      <section className="space-y-2"><h3 className="text-sm font-medium">节点 Realm 流量</h3><p className="text-sm">{!host.online ? "节点离线" : !host.traffic_capable ? "需要升级 Agent 以监控" : trafficState(stats)}</p><TrafficSummary items={traffic.rules.filter((r) => r.node_id === host.node_id)} status={stats} /><p className="text-xs text-muted-foreground">采集缺口：{stats?.gaps ?? "—"} · 无法归属记录：{stats?.discarded_records ?? 0} · 最近采集：{stats?.last_sample ? new Date(stats.last_sample * 1000).toLocaleString() : "—"}</p><p className="text-xs text-muted-foreground">入口端口流量仅统计客户端侧 TCP，包含协议开销和重传，排除 loopback；与整机网卡分开，不用于用户扣费。今日 / 本月按 UTC，缺失时段不补算。</p></section>
      <form className="space-y-3 rounded-md border p-3" onSubmit={(e) => { e.preventDefault(); if (!changed || !confirmed || busy) return; void perform(() => api("/realm/hosts", { method: "PUT", body: JSON.stringify({ node_id: host.node_id, public_address: address.trim(), enabled, takeover: true } satisfies RealmHostDraft) }), "已保存入口节点共享设置") }}>
        <Field title="对外连接地址（不含端口）"><Input aria-label="对外连接地址（不含端口）" required value={address} onChange={(e) => { setAddress(e.target.value); setConfirmed(false) }} /></Field>
        <label className="flex gap-2 text-sm"><input type="checkbox" checked={enabled} onChange={(e) => { setEnabled(e.target.checked); setConfirmed(false) }} />启用入口 Realm 服务</label>
        {changed && <label className="flex gap-2 text-sm"><input type="checkbox" required checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} />确认修改节点共享设置，影响全部 Realm 规则</label>}
        <Button size="sm" disabled={!changed || !confirmed || busy}>保存共享设置</Button>
      </form>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void perform(() => operation("retry"), "已请求刷新状态并重试应用")}>刷新 / 重试</Button>
        <Button size="sm" variant="outline" disabled={busy || !host.online || !host.traffic_capable} onClick={() => void perform(() => operation("traffic_recheck"), "已请求重新检测 nft 流量监控")}>重新检测流量监控</Button>
        <Button size="sm" variant="outline" disabled={busy || !host.enabled || !host.status.running} onClick={() => { if (confirm("重启入口 Realm 服务？此节点全部 Realm 连接会暂时中断。")) void perform(() => operation("restart"), "已请求重启") }}>重启入口服务</Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void perform(async () => { const data = await api<{ desired: unknown }>(`/realm/hosts/${host.node_id}/config`); setPreview(JSON.stringify(data.desired, null, 2)) }, "已读取配置预览")}>配置预览</Button>
        <Button size="sm" variant="outline" disabled={busy || rules.some((r) => r.node_id === host.node_id) || host.status.running !== false || host.pending} onClick={() => { if (confirm("解除此节点的 Realm 面板管理？节点本身、Realm 程序和历史流量保留。")) void perform(async () => { await api(`/realm/hosts/${host.node_id}`, { method: "DELETE" }); onClose() }, "已解除 Realm 面板管理") }}>解除面板管理</Button>
      </div>
      <InstallCommand nodeId={host.node_id} site={site} refusal={refusal} />
      {preview && <pre className="max-h-64 overflow-auto rounded border p-3 text-xs">{preview}</pre>}
    </div>
  </DialogContent></Dialog>
}
