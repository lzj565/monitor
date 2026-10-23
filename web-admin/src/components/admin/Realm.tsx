import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { api, type Node } from "@/lib/api"
import type { Proxy } from "@/lib/resources"
import { trafficBytes, trafficState, type Traffic, type TrafficRule, type TrafficStatus } from "@/lib/realm-traffic"
import { installCommand } from "./Nodes"

type Host = { node_id: number; name: string; public_address: string; enabled: boolean; desired_revision: number; applied_revision: number; pending: boolean; online: boolean; capable: boolean; traffic_capable: boolean; last_error: string; status: { installed?: boolean; managed?: boolean; version?: string; running?: boolean; enabled?: boolean; target_reachability?: string; targets_checked?: number; targets_total?: number } }
type HostDraft = { node_id: number; public_address: string; enabled: boolean; takeover: boolean }
type Rule = { traffic_key: string; id: number; node_id: number; target_proxy_id: number; name: string; listen_host: string; listen_port: number; enabled: boolean; target_node_id: number; target_name: string; target_address: string; target_port: number; target_enabled: boolean }
type RuleDraft = Pick<Rule, "node_id" | "target_proxy_id" | "name" | "listen_host" | "listen_port" | "enabled">
const selectClass = "h-9 rounded-md border bg-background px-3 text-sm"
const initialRule: RuleDraft = { node_id: 0, target_proxy_id: 0, name: "", listen_host: "0.0.0.0", listen_port: 0, enabled: true }
function status(host: Host) {
  if (!host.online) return "节点离线，等待重连"
  if (!host.capable) return "需要升级 Agent"
  if (host.status.installed === false) return "未安装 Realm"
  if (host.status.managed === false) return "不是本安装器管理的 Realm"
  if (host.last_error) return "应用失败 / 等待重试"
  if (host.pending) return "待应用"
  if (host.status.running) return "运行中"
  return host.status.running === false ? "已停止" : "读取状态中"
}

export function Realm({ nodes, site, refusal }: { nodes: Node[]; site: string; refusal: string }) {
  const [traffic, setTraffic] = useState<Traffic>({ hosts: [], rules: [] })
  const [hosts, setHosts] = useState<Host[]>([])
  const [rules, setRules] = useState<Rule[]>([])
  const [proxies, setProxies] = useState<Proxy[]>([])
  const [error, setError] = useState("")
  const [hostForm, setHostForm] = useState<HostDraft | null>(null)
  const [ruleForm, setRuleForm] = useState<RuleDraft | null>(null)
  const [ruleId, setRuleId] = useState<number | null>(null)
  const [targetNode, setTargetNode] = useState(0)
  const [busy, setBusy] = useState(false)
  const [preview, setPreview] = useState("")
  const [installation, setInstallation] = useState("")
  const load = useCallback(async (signal?: AbortSignal) => {
    const [h, r, p, t] = await Promise.all([
      api<{ items: Host[] }>("/realm/hosts", { signal }),
      api<{ items: Rule[] }>("/realm/rules", { signal }),
      api<{ items: Proxy[] }>("/proxies", { signal }),
      api<Traffic>("/realm/traffic", { signal }),
    ])
    setTraffic(t); setHosts(h.items); setRules(r.items); setProxies(p.items); setError("")
  }, [])
  useEffect(() => {
    const controller = new AbortController()
    let pending = false
    const refresh = async () => {
      if (pending || document.hidden) return
      pending = true
      try { await load(controller.signal) } catch (e) { if (!controller.signal.aborted) setError((e as Error).message) }
      finally { pending = false }
    }
    void refresh()
    const timer = setInterval(refresh, 5000)
    document.addEventListener("visibilitychange", refresh)
    return () => { controller.abort(); clearInterval(timer); document.removeEventListener("visibilitychange", refresh) }
  }, [load])
  const perform = async (work: () => Promise<unknown>, message: string) => {
    setBusy(true)
    try { await work(); toast.success(message); await load() } catch (e) { toast.error((e as Error).message) }
    finally { setBusy(false) }
  }
  const chooseHost = (id: number) => {
    const node = nodes.find((n) => n.id === id)
    const address = node?.addresses?.find((a) => a.address)?.address ?? ""
    setHostForm({ node_id: id, public_address: address, enabled: true, takeover: false })
  }
  const install = async (id: number) => {
    try {
      const { token } = await api<{ token: string }>(`/nodes/${id}/token`)
      setInstallation(installCommand(site, token, undefined, undefined, true))
    } catch (e) { toast.error((e as Error).message) }
  }
  const openRule = (rule?: Rule) => {
    setRuleId(rule?.id ?? null)
    setRuleForm(rule ? { node_id: rule.node_id, target_proxy_id: rule.target_proxy_id, name: rule.name, listen_host: rule.listen_host, listen_port: rule.listen_port, enabled: rule.enabled } : { ...initialRule, node_id: hosts[0]?.node_id ?? 0, listen_host: hosts[0]?.public_address.includes(":") ? "::" : "0.0.0.0" })
    setTargetNode(rule?.target_node_id ?? 0)
  }
  return <div className="space-y-6">
    <div><h2 className="text-xl font-semibold">Realm 转发</h2><p className="mt-1 text-sm text-muted-foreground">用户 → 入口机器的监听端口 → 目标节点上的代理。保存后自动应用，成功后加入用户订阅；直连入口保留。</p></div>
    {error && <p role="alert" className="text-sm text-destructive">读取失败：{error}，保留上次结果并自动重试。</p>}
    <p className="text-xs text-muted-foreground">入口端口流量：仅统计客户端侧 TCP，包含协议开销和重传，排除 loopback。与整机网卡流量分开，不用于用户扣费。今日 / 本月按 UTC；缺失时段不补算。</p>
    <Card className="space-y-4 p-4">
      <div className="flex items-center justify-between"><h3 className="font-medium">Realm 机器</h3><Button disabled={busy} onClick={() => chooseHost(nodes.find((n) => !hosts.some((h) => h.node_id === n.id))?.id ?? 0)}>添加机器</Button></div>
      {!hosts.length && <p className="text-sm text-muted-foreground">从已有监控节点中选择入口机器。节点需安装支持 Realm 管理的新 Agent。</p>}
      {hosts.map((host) => <div key={host.node_id} className="space-y-2 rounded-md border p-3">
        <div className="flex flex-wrap items-center gap-3"><strong>{host.name}</strong><span className="text-sm">{host.public_address}</span><span className="text-sm">{status(host)}</span><span className="text-xs text-muted-foreground">期望 {host.desired_revision} / 已应用 {host.applied_revision} · {host.status.version ?? "版本未知"}</span></div>
        <p className="text-xs text-muted-foreground">目标可达：{host.status.target_reachability === "reachable" ? "已检测目标可连接" : host.status.target_reachability === "unreachable" ? "检测目标不可连接" : host.status.target_reachability === "partial" ? "部分可连接 / 部分未检测" : "未检测"}{host.status.targets_total ? `（${host.status.targets_checked ?? 0}/${host.status.targets_total}）` : ""}。运行状态不代表完整代理链路可用。</p>
        <p className="text-sm">流量：{!host.online ? "节点离线" : !host.traffic_capable ? "需要升级 Agent 以监控" : trafficState(traffic.hosts.find((h) => h.node_id === host.node_id)?.status)}</p>
        <TrafficSummary items={traffic.rules.filter((r) => r.node_id === host.node_id)} status={traffic.hosts.find((h) => h.node_id === host.node_id)?.status} />
        <p className="text-xs text-muted-foreground">采集缺口次数：{traffic.hosts.find((h) => h.node_id === host.node_id)?.status.gaps ?? "—"} · 无法归属记录：{traffic.hosts.find((h) => h.node_id === host.node_id)?.status.discarded_records ?? 0} · 最近采集：{traffic.hosts.find((h) => h.node_id === host.node_id)?.status.last_sample ? new Date(traffic.hosts.find((h) => h.node_id === host.node_id)!.status.last_sample! * 1000).toLocaleString() : "—"}</p>
        {host.last_error && <p role="alert" className="break-all text-sm text-destructive">{host.last_error}</p>}
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="outline" disabled={busy} onClick={() => setHostForm({ node_id: host.node_id, public_address: host.public_address, enabled: host.enabled, takeover: true })}>编辑</Button>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => perform(() => api("/realm/hosts", { method: "PUT", body: JSON.stringify({ node_id: host.node_id, public_address: host.public_address, enabled: !host.enabled, takeover: true }) }), host.enabled ? "已保存停用，等待 Agent 停止服务" : "已保存启用，等待应用")}>{host.enabled ? "停用" : "启用"}</Button>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => perform(() => api(`/realm/hosts/${host.node_id}/operations`, { method: "POST", body: JSON.stringify({ action: "retry" }) }), "已请求刷新状态并重试应用")}>刷新 / 重试</Button>
          <Button size="sm" variant="outline" disabled={busy || !host.online || !host.traffic_capable} onClick={() => perform(() => api(`/realm/hosts/${host.node_id}/operations`, { method: "POST", body: JSON.stringify({ action: "traffic_recheck" }) }), "已请求重新检测 nft 流量监控")}>重新检测流量监控</Button>
          <Button size="sm" variant="outline" disabled={busy || !host.enabled || !host.status.running} onClick={() => perform(() => api(`/realm/hosts/${host.node_id}/operations`, { method: "POST", body: JSON.stringify({ action: "restart" }) }), "已请求重启")}>重启</Button>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => perform(async () => { const data = await api<{ desired: unknown }>(`/realm/hosts/${host.node_id}/config`); setPreview(JSON.stringify(data.desired, null, 2)) }, "已读取配置预览")}>配置预览</Button>
          <Button size="sm" variant="outline" disabled={busy || !!refusal} title={refusal} onClick={() => void install(host.node_id)}>安装命令</Button>
          <Button size="sm" variant="outline" disabled={busy || rules.some((r) => r.node_id === host.node_id) || host.status.running !== false || host.pending} onClick={() => { if (confirm("移除这台 Realm 机器？保留机器上的程序和配置。")) void perform(() => api(`/realm/hosts/${host.node_id}`, { method: "DELETE" }), "已移除 Realm 机器") }}>移除</Button>
        </div>
      </div>)}
      {hostForm && <form className="space-y-3 rounded-md border p-4" onSubmit={(e) => { e.preventDefault(); void perform(async () => { await api("/realm/hosts", { method: "PUT", body: JSON.stringify(hostForm) }); setHostForm(null) }, "机器已保存，等待自动同步") }}>
        <label className="block text-sm">入口节点<select className={`${selectClass} ml-3`} value={hostForm.node_id} disabled={hosts.some((h) => h.node_id === hostForm.node_id)} onChange={(e) => chooseHost(Number(e.target.value))}><option value={0}>选择节点</option>{nodes.filter((n) => n.id === hostForm.node_id || !hosts.some((h) => h.node_id === n.id)).map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}</select></label>
        <label className="block space-y-1 text-sm"><span>对外域名 / IP（不含端口）</span><Input required value={hostForm.public_address} onChange={(e) => setHostForm({ ...hostForm, public_address: e.target.value.trim() })} /></label>
        <label className="flex gap-2 text-sm"><input type="checkbox" checked={hostForm.enabled} onChange={(e) => setHostForm({ ...hostForm, enabled: e.target.checked })} />启用机器，有有效规则后自动启动并设置开机启动</label>
        <label className="flex gap-2 text-sm"><input type="checkbox" required checked={hostForm.takeover} onChange={(e) => setHostForm({ ...hostForm, takeover: e.target.checked })} />确认由面板接管本安装器的 Realm 配置，首次应用前备份原配置；第三方安装不会被接管。</label>
        <div className="flex gap-2"><Button disabled={busy || !hostForm.node_id || !hostForm.takeover}>保存</Button><Button type="button" variant="outline" onClick={() => setHostForm(null)}>取消</Button></div>
      </form>}
    </Card>
    <Card className="space-y-4 p-4">
      <div className="flex items-center justify-between"><h3 className="font-medium">转发规则</h3><Button disabled={busy || !hosts.length} onClick={() => openRule()}>新建规则</Button></div>
      {!rules.length && <p className="text-sm text-muted-foreground">选择入口机器和目标节点下的代理，填写入口监听端口。</p>}
      {rules.map((rule) => <div key={rule.id} className="space-y-2 rounded-md border p-3"><div className="flex flex-wrap gap-3"><strong>{rule.name}</strong><span className="text-sm">{hosts.find((h) => h.node_id === rule.node_id)?.name}:{rule.listen_port} → {rule.target_name} ({rule.target_address}:{rule.target_port})</span><span className="text-sm">{!rule.enabled ? "已停用" : !rule.target_enabled ? "目标代理已停用" : "已启用"}</span></div><TrafficSummary items={traffic.rules.filter((r) => r.key === rule.traffic_key)} /><div className="flex gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => openRule(rule)}>编辑</Button><Button size="sm" variant="outline" disabled={busy} onClick={() => { if (confirm("删除这条转发规则？订阅立即移除，Agent 随后更新配置。")) void perform(() => api(`/realm/rules/${rule.id}`, { method: "DELETE" }), "已删除规则，等待同步") }}>删除</Button></div></div>)}
      {ruleForm && <form className="grid gap-3 rounded-md border p-4 sm:grid-cols-2" onSubmit={(e) => { e.preventDefault(); void perform(async () => { await api(ruleId ? `/realm/rules/${ruleId}` : "/realm/rules", { method: ruleId ? "PUT" : "POST", body: JSON.stringify(ruleForm) }); setRuleForm(null) }, "规则已保存，应用成功后发布订阅入口") }}>
        <label className="space-y-1 text-sm"><span>规则名称</span><Input required maxLength={128} value={ruleForm.name} onChange={(e) => setRuleForm({ ...ruleForm, name: e.target.value })} /></label>
        <label className="flex flex-col gap-1 text-sm">入口机器<select className={selectClass} value={ruleForm.node_id} onChange={(e) => { setRuleForm({ ...ruleForm, node_id: Number(e.target.value), listen_host: hosts.find((h) => h.node_id === Number(e.target.value))?.public_address.includes(":") ? "::" : "0.0.0.0", target_proxy_id: 0 }); setTargetNode(0) }}>{hosts.map((h) => <option key={h.node_id} value={h.node_id}>{h.name}</option>)}</select></label>
        <label className="flex flex-col gap-1 text-sm">监听地址<select className={selectClass} value={ruleForm.listen_host} onChange={(e) => setRuleForm({ ...ruleForm, listen_host: e.target.value })}><option value="0.0.0.0">IPv4 · 0.0.0.0</option><option value="::">IPv6 · ::</option></select></label>
        <label className="space-y-1 text-sm"><span>监听端口</span><Input type="number" required min={1} max={65535} value={ruleForm.listen_port || ""} onChange={(e) => setRuleForm({ ...ruleForm, listen_port: Number(e.target.value) })} /></label>
        <label className="flex flex-col gap-1 text-sm">目标节点<select className={selectClass} value={targetNode} onChange={(e) => { setTargetNode(Number(e.target.value)); setRuleForm({ ...ruleForm, target_proxy_id: 0 }) }}><option value={0}>选择节点</option>{nodes.filter((n) => n.id !== ruleForm.node_id).map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}</select></label>
        <label className="flex flex-col gap-1 text-sm">目标代理<select required className={selectClass} value={ruleForm.target_proxy_id || ""} onChange={(e) => setRuleForm({ ...ruleForm, target_proxy_id: Number(e.target.value) })}><option value="">选择已有代理</option>{proxies.filter((p) => p.node_id === targetNode && (p.enabled || p.id === ruleForm.target_proxy_id)).map((p) => <option key={p.id} value={p.id}>{p.name} · {p.address}:{p.port}{p.enabled ? "" : "（已停用）"}</option>)}</select></label>
        <label className="flex gap-2 text-sm"><input type="checkbox" checked={ruleForm.enabled} onChange={(e) => setRuleForm({ ...ruleForm, enabled: e.target.checked })} />启用规则</label>
        <div className="flex gap-2"><Button disabled={busy || !ruleForm.target_proxy_id || !ruleForm.node_id}>保存并应用</Button><Button type="button" variant="outline" onClick={() => setRuleForm(null)}>取消</Button></div>
      </form>}
    </Card>
    {traffic.rules.some((r) => !r.active) && <Card className="space-y-3 p-4"><h3>已删除规则的历史流量</h3>{traffic.rules.filter((r) => !r.active).map((r) => <div key={r.key}><span>{r.name}</span><TrafficSummary items={[r]} /></div>)}</Card>}
    {installation && <Card className="space-y-2 p-4"><div className="flex justify-between"><h3 className="font-medium">在入口机器执行安装命令</h3><Button size="sm" variant="outline" onClick={() => setInstallation("")}>关闭</Button></div><p className="text-sm text-muted-foreground">执行完成后点击刷新。仍需升级旧 Agent，安装 Realm 本身不会增加 Agent 管理能力。</p><textarea aria-label="Realm 安装命令" readOnly className="min-h-24 w-full rounded border bg-muted p-3 font-mono text-xs" value={installation} /></Card>}
    {preview && <Card className="p-4"><div className="flex justify-between"><h3>期望配置预览</h3><Button size="sm" variant="outline" onClick={() => setPreview("")}>关闭</Button></div><pre className="mt-3 overflow-auto text-xs">{preview}</pre></Card>}
  </div>
}

function TrafficSummary({ items, status }: { items: TrafficRule[]; status?: TrafficStatus }) {
  const known = items.some((r) => r.last_sample)
  const sum = (field: "up" | "down" | "today" | "month") => known ? items.reduce((n, r) => n + r[field], 0) : null
  const live = items.length > 0 && items.every((r) => r.up_rate != null && r.down_rate != null)
  const upRate = status ? status.fresh ? status.up_rate : null : live ? items.reduce((n, r) => n + r.up_rate!, 0) : null
  const downRate = status ? status.fresh ? status.down_rate : null : live ? items.reduce((n, r) => n + r.down_rate!, 0) : null
  return <p className="text-xs text-muted-foreground">累计 ↑ {trafficBytes(sum("up"))} / ↓ {trafficBytes(sum("down"))} · 今日 {trafficBytes(sum("today"))} · 本月 {trafficBytes(sum("month"))} · 速率 ↑ {trafficBytes(upRate)}/s / ↓ {trafficBytes(downRate)}/s</p>
}
