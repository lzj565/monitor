import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Badge } from "@/components/ui/badge"
import { Switch } from "@/components/ui/switch"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Table, TableBody, TableHead, TableHeader, TableCell, TableRow } from "@/components/ui/table"

export type ExternalProxy = { id: number; name: string; protocol: string; address: string | null; port: number | null; enabled: boolean }
type Config = Record<string, unknown>
type Preview = { index: number; config: Config; error: string | null; warning?: string | null }
const changed = () => window.dispatchEvent(new Event("external-proxies-changed"))

export function useExternalProxies() {
  const [items, setItems] = useState<ExternalProxy[]>([])
  const [error, setError] = useState("")
  const [loading, setLoading] = useState(true)
  const reload = useCallback(() => {
    return api<{ items: ExternalProxy[] }>("/external-proxies").then(r => { setItems(r.items); setError("") }, (e: Error) => setError(e.message)).finally(() => setLoading(false))
  }, [])
  useEffect(() => { void reload(); const update = () => { void reload() }; window.addEventListener("external-proxies-changed", update); return () => window.removeEventListener("external-proxies-changed", update) }, [reload])
  return { items, error, loading, reload }
}

export function ExternalImportDialog({ editing, onClose }: { editing?: ExternalProxy; onClose: () => void }) {
  const [yaml, setYaml] = useState("")
  const [rows, setRows] = useState<Preview[] | null>(null)
  const [dirty, setDirty] = useState(false)
  const [ignored, setIgnored] = useState(false)
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(!!editing)
  const [error, setError] = useState("")
  const load = useCallback(() => {
    if (!editing) return
    void api<{ config: Config }>(`/external-proxies/${editing.id}`).then(r => { setYaml(JSON.stringify([r.config], null, 2)); setError("") }).catch((e: Error) => setError(e.message)).finally(() => setLoading(false))
  }, [editing])
  useEffect(load, [load])
  async function preview() {
    setBusy(true); setError("")
    try {
      const source = dirty && rows ? JSON.stringify(rows.map(r => r.config)) : yaml
      const r = await api<{ items: Preview[]; ignored_sections: boolean }>("/external-proxies/preview", { method: "POST", body: JSON.stringify({ yaml: source, exclude_id: editing?.id }) })
      setRows(r.items); setDirty(false); setIgnored(r.ignored_sections)
      if (editing && r.items.length !== 1) setError("编辑时只能保留一条代理配置")
    } catch (e) { setRows(null); setError((e as Error).message) }
    finally { setBusy(false) }
  }
  async function save() {
    if (!rows || dirty || rows.some(r => r.error) || (editing && rows.length !== 1)) return
    setBusy(true); setError("")
    try {
      await api(editing ? `/external-proxies/${editing.id}` : "/external-proxies/import", { method: editing ? "PUT" : "POST", body: JSON.stringify(editing ? { config: rows[0].config } : { items: rows.map(r => r.config) }) })
      changed(); toast.success(editing ? "外部代理已更新" : `已导入 ${rows.length} 条外部代理，尚未授权给用户`); onClose()
    } catch (e) { setError((e as Error).message) }
    finally { setBusy(false) }
  }
  return <Dialog open onOpenChange={open => !open && !busy && onClose()}><DialogContent className="flex max-h-[90dvh] flex-col sm:max-w-3xl">
    <DialogHeader><DialogTitle>{editing ? `编辑外部代理：${editing.name}` : "导入外部代理"}</DialogTitle><DialogDescription>粘贴单个代理、完整 Mihomo 配置或静态代理列表。仅导入代理，不导入分组、规则或远程 providers。保留原凭据，仅用于 Mihomo，不采集统计。校验不代表连接实测。</DialogDescription></DialogHeader>
    <div className="min-h-0 space-y-3 overflow-y-auto">
      {error && <div role="alert" className="text-sm text-destructive">{error}{editing && !yaml && <Button variant="link" onClick={load}>重试读取</Button>}</div>}
      <label className="block space-y-2"><span className="text-sm font-medium">YAML 配置（最多 1 MiB / 500 条）</span><textarea aria-label="YAML 配置" className="min-h-52 w-full rounded-md border bg-background p-3 font-mono text-xs" value={yaml} disabled={busy || loading} onChange={e => { setYaml(e.target.value); setRows(null); setDirty(false); setIgnored(false); setError("") }} placeholder={'proxies:\n  - name: 示例\n    type: trojan\n    server: example.com\n    port: 443\n    password: your-password'} /></label>
      {ignored && <p className="text-sm text-muted-foreground">仅导入 proxies，其他配置未导入。</p>}
      {rows && <div className="space-y-2">{rows.map((r, i) => <div key={r.index} className="space-y-2 rounded-md border p-3"><label className="flex items-center gap-2 text-sm"><span className="shrink-0">第 {i + 1} 条</span><Input aria-label={`第 ${i + 1} 条名称`} disabled={busy} value={typeof r.config?.name === "string" ? r.config.name : ""} onChange={e => { setRows(rows.map((row, j) => j === i ? { ...row, config: { ...row.config, name: e.target.value } } : row)); setDirty(true) }} /></label><p className="break-all text-xs text-muted-foreground">{String(r.config?.type ?? "未知协议")} · {String(r.config?.server ?? "缺少地址")}:{String(r.config?.port ?? "—")}</p>{r.warning && <p className="text-sm text-muted-foreground">{r.warning}</p>}{r.error && <p role="alert" className="text-sm text-destructive">{r.error}</p>}</div>)}</div>}
    </div>
    <DialogFooter className="flex-wrap"><Button variant="ghost" disabled={busy} onClick={onClose}>取消</Button><Button variant="outline" disabled={busy || loading || !yaml.trim()} onClick={() => void preview()}>{busy ? "处理中…" : rows ? "重新校验" : "解析预览"}</Button><Button disabled={busy || loading || !rows || dirty || !!error || rows.some(r => r.error !== null)} onClick={() => void save()}>{editing ? "保存配置" : "确认导入"}</Button></DialogFooter>
  </DialogContent></Dialog>
}

export function ExternalProxyRow({ proxy }: { proxy: ExternalProxy }) {
  const [editing, setEditing] = useState(false)
  const [busy, setBusy] = useState(false)
  async function mutate(remove: boolean, enabled?: boolean) {
    if (remove && !window.confirm(`删除外部代理「${proxy.name}」及所有用户授权和偏好？外部服务的凭据不会失效。`)) return
    setBusy(true)
    try { await api(`/external-proxies/${proxy.id}`, { method: remove ? "DELETE" : "PUT", body: remove ? undefined : JSON.stringify({ enabled }) }); changed(); toast.success(remove ? "外部代理已删除" : "状态已更新") }
    catch (e) { toast.error((e as Error).message) }
    finally { setBusy(false) }
  }
  return <TableRow><TableCell className="max-w-64 break-all font-medium">{proxy.name}</TableCell><TableCell><Badge variant="outline">外部导入 · {proxy.protocol}</Badge></TableCell><TableCell className="font-mono text-xs">{proxy.address ?? "—"}</TableCell><TableCell>{proxy.port ?? "—"}</TableCell><TableCell><Switch checked={proxy.enabled} disabled={busy} aria-label={`${proxy.enabled ? "停用" : "启用"}外部代理${proxy.name}`} onCheckedChange={v => void mutate(false, v)} /></TableCell><TableCell><div className="flex justify-end gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => setEditing(true)}>编辑配置</Button><Button size="sm" variant="ghost" disabled={busy} onClick={() => void mutate(true)}>删除</Button></div>{editing && <ExternalImportDialog editing={proxy} onClose={() => setEditing(false)} />}</TableCell></TableRow>
}

export function ExternalAuthorization({ userId }: { userId: number }) {
  const { items, loading, error, reload } = useExternalProxies()
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [loaded, setLoaded] = useState(false)
  const [accessError, setAccessError] = useState("")
  const [saving, setSaving] = useState(false)
  const [query, setQuery] = useState("")
  const loadAccess = useCallback(() => {
    void api<{ ids: number[] }>(`/users/${userId}/external-proxies`).then(r => { setSelected(new Set(r.ids)); setLoaded(true); setAccessError("") }).catch((e: Error) => setAccessError(e.message))
  }, [userId])
  useEffect(loadAccess, [loadAccess])
  async function save() {
    setSaving(true)
    try { await api(`/users/${userId}/external-proxies`, { method: "PUT", body: JSON.stringify({ ids: [...selected] }) }); window.dispatchEvent(new Event("external-authorizations-changed")); toast.success("外部代理授权已保存") }
    catch (e) { toast.error((e as Error).message) }
    finally { setSaving(false) }
  }
  return <section className="mt-4 space-y-3 rounded-md border p-4" aria-label="外部代理授权"><h3 className="text-sm font-semibold">外部代理授权 · 独立保存</h3><p className="text-xs text-muted-foreground">仅进入 Mihomo 订阅，共用导入凭据，不采集统计。撤销授权不会让已复制的凭据失效。自动授权新增代理不包含外部导入。</p>
    {(error || accessError) && <p role="alert" className="text-sm text-destructive">{error || accessError}<Button type="button" variant="link" onClick={() => { void reload(); loadAccess() }}>重试</Button></p>}
    <Input aria-label="搜索外部代理授权" placeholder="搜索外部代理名称或地址" value={query} onChange={e => setQuery(e.target.value)} />
    <div className="space-y-2">{items.filter(p => `${p.name} ${p.address}`.toLowerCase().includes(query.trim().toLowerCase())).map(p => <label key={p.id} className="flex items-center gap-3 text-sm"><input type="checkbox" checked={selected.has(p.id)} disabled={!loaded || saving || loading} onChange={e => setSelected(current => { const next = new Set(current); if (e.target.checked) next.add(p.id); else next.delete(p.id); return next })} /><span className="min-w-0 break-all">{p.name} · {p.protocol} · {p.address}:{p.port}{!p.enabled && "（已停用）"}</span></label>)}</div>
    {!loading && !items.length && !error && <p className="text-sm text-muted-foreground">尚无外部代理，请先在“代理”页面导入。</p>}
    <Button type="button" variant="ghost" disabled={saving || loading} onClick={loadAccess}>重新读取授权</Button>
    <Button type="button" variant="outline" disabled={saving || !loaded || loading || !!error} onClick={() => void save()}>{saving ? "保存中…" : `保存外部授权（${selected.size}）`}</Button>
  </section>
}

export function ExternalAuthorizedSummary({ userId }: { userId: number }) {
  const [items, setItems] = useState<ExternalProxy[]>([])
  const [error, setError] = useState("")
  const [loading, setLoading] = useState(true)
  useEffect(() => {
    let active = true
    const load = () => { void Promise.all([api<{ items: ExternalProxy[] }>("/external-proxies"), api<{ ids: number[] }>(`/users/${userId}/external-proxies`)]).then(([all, access]) => { if (active) { setItems(all.items.filter(p => access.ids.includes(p.id))); setError("") } }).catch((e: Error) => { if (active) setError(e.message) }).finally(() => { if (active) setLoading(false) }) }
    load(); window.addEventListener("external-proxies-changed", load); window.addEventListener("external-authorizations-changed", load)
    return () => { active = false; window.removeEventListener("external-proxies-changed", load); window.removeEventListener("external-authorizations-changed", load) }
  }, [userId])
  return <div className="space-y-2 border-t p-4"><h4 className="text-sm font-medium">外部代理授权（{items.length}）</h4>{loading ? <p className="text-xs text-muted-foreground">读取中…</p> : error ? <p role="alert" className="text-sm text-destructive">{error}</p> : items.length ? items.map(p => <p key={p.id} className="break-all text-sm">{p.name} · {p.protocol} · {p.address}:{p.port}{!p.enabled && "（已停用）"}</p>) : <p className="text-xs text-muted-foreground">没有外部授权</p>}</div>
}

export function ExternalProxiesPage() {
  const { items, loading, error, reload } = useExternalProxies()
  const [query, setQuery] = useState("")
  const [status, setStatus] = useState("all")
  const [importing, setImporting] = useState(false)
  const visible = items.filter(p => `${p.name} ${p.protocol} ${p.address}`.toLowerCase().includes(query.trim().toLowerCase()) && (status === "all" || p.enabled === (status === "enabled")))
  return <div className="space-y-4">
    <div><h1 className="text-lg font-semibold">外部代理</h1><p className="mt-1 text-sm text-muted-foreground">独立管理导入的 Mihomo 代理，不关联节点、不采集统计。在“订阅”中为用户分配授权。</p></div>
    <div className="flex flex-wrap items-center gap-2"><Input className="min-w-40 flex-1" aria-label="搜索外部代理" placeholder="搜索名称、协议或地址" value={query} onChange={e => setQuery(e.target.value)} /><select aria-label="外部代理状态筛选" className="h-9 rounded-md border bg-background px-3 text-sm" value={status} onChange={e => setStatus(e.target.value)}><option value="all">全部状态</option><option value="enabled">已启用</option><option value="disabled">已停用</option></select><Button onClick={() => setImporting(true)}>导入外部代理</Button></div>
    {error && <div role="alert" className="text-sm text-destructive">加载外部代理失败：{error}<Button variant="link" onClick={() => void reload()}>重试</Button></div>}
    <div className="overflow-x-auto rounded-md border"><Table><TableHeader><TableRow>{["名称", "协议", "地址", "端口", "启用状态", "操作"].map(label => <TableHead key={label}>{label}</TableHead>)}</TableRow></TableHeader><TableBody>{loading ? <TableRow><TableCell colSpan={6}>读取中…</TableCell></TableRow> : visible.map(proxy => <ExternalProxyRow key={proxy.id} proxy={proxy} />)}{!loading && !error && !visible.length && <TableRow><TableCell colSpan={6} className="py-10 text-center text-muted-foreground">{items.length ? "没有匹配的外部代理" : "尚无外部代理，可直接导入，无需添加节点。"}</TableCell></TableRow>}</TableBody></Table></div>
    {importing && <ExternalImportDialog onClose={() => setImporting(false)} />}
  </div>
}
