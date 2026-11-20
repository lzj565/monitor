import { useCallback, useEffect, useState } from "react"
import { createPortal } from "react-dom"
import { Pencil, Trash2, Upload } from "lucide-react"
import { toast } from "sonner"
import { api } from "@/lib/api"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Card } from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import { AdminSearchInput, StatusFilter } from "@/components/AdminShared"
import { ProxyProtocolBadge, TooltipText } from "@/components/ProxyBadges"
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
  return <Dialog open onOpenChange={open => !open && !busy && onClose()}><DialogContent className="flex max-h-[90dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-3xl">
    <DialogHeader className="shrink-0 border-b px-6 py-4"><DialogTitle>{editing ? `编辑外部代理：${editing.name}` : "导入外部代理"}</DialogTitle><DialogDescription>粘贴单个代理、完整 Mihomo 配置或静态代理列表，支持 Snell v6。仅导入代理，不导入分组、规则或远程 providers。保留原凭据，仅用于 Mihomo，不采集统计。校验不代表连接实测。</DialogDescription></DialogHeader>
    <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-6 py-4">
      {error && <div role="alert" className="text-sm text-destructive">{error}{editing && !yaml && <Button variant="link" onClick={load}>重试读取</Button>}</div>}
      <label className="block space-y-2"><span className="text-sm font-medium">YAML / JSON 配置（最多 1 MiB / 500 条）</span><textarea aria-label="YAML / JSON 配置" className="min-h-52 w-full resize-y rounded-md border bg-muted/30 p-3 font-mono text-sm leading-relaxed" value={yaml} disabled={busy || loading} onChange={e => { setYaml(e.target.value); setRows(null); setDirty(false); setIgnored(false); setError("") }} placeholder={'proxies:\n  - name: 示例\n    type: trojan\n    server: example.com\n    port: 443\n    password: your-password'} /></label>
      {ignored && <p className="text-sm text-muted-foreground">仅导入 proxies，其他配置未导入。</p>}
      {rows && <div className="space-y-2">{rows.map((r, i) => <div key={r.index} className="space-y-2 rounded-md border p-3"><label className="flex items-center gap-2 text-sm"><span className="shrink-0">第 {i + 1} 条</span><Input aria-label={`第 ${i + 1} 条名称`} disabled={busy} value={typeof r.config?.name === "string" ? r.config.name : ""} onChange={e => { setRows(rows.map((row, j) => j === i ? { ...row, config: { ...row.config, name: e.target.value } } : row)); setDirty(true) }} /></label><p className="break-all text-xs text-muted-foreground">{String(r.config?.type ?? "未知协议")} · {String(r.config?.server ?? "缺少地址")}:{String(r.config?.port ?? "—")}</p>{r.warning && <p className="text-sm text-muted-foreground">{r.warning}</p>}{r.error && <p role="alert" className="text-sm text-destructive">{r.error}</p>}</div>)}</div>}
    </div>
    <DialogFooter className="shrink-0 flex-wrap border-t px-6 py-3"><Button variant="ghost" disabled={busy} onClick={onClose}>取消</Button><Button variant="outline" disabled={busy || loading || !yaml.trim()} onClick={() => void preview()}>{busy ? "处理中…" : rows ? "重新校验" : "解析预览"}</Button><Button disabled={busy || loading || !rows || dirty || !!error || rows.some(r => r.error !== null)} onClick={() => void save()}>{editing ? "保存配置" : "确认导入"}</Button></DialogFooter>
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
  return (
    <TableRow className="h-14">
      <TableCell className="px-3 font-medium"><TooltipText text={proxy.name} className="max-w-64" /></TableCell>
      <TableCell className="px-3"><ProxyProtocolBadge protocol={proxy.protocol} label={proxy.protocol} /></TableCell>
      <TableCell className="px-3"><TooltipText text={proxy.address ?? "—"} className="max-w-48 text-sm" /></TableCell>
      <TableCell className="tnum px-3 text-sm">{proxy.port ?? "—"}</TableCell>
      <TableCell className="px-3 text-center">
        <Switch checked={proxy.enabled} disabled={busy} aria-label={`${proxy.enabled ? "停用" : "启用"}外部代理 ${proxy.name}`} onCheckedChange={enabled => void mutate(false, enabled)} />
      </TableCell>
      <TableCell className="whitespace-nowrap px-3 text-right">
        <div className="flex items-center justify-end gap-1">
          <Button size="icon" variant="ghost" className="max-md:size-8" title="编辑配置" aria-label={`编辑 ${proxy.name}`} disabled={busy} onClick={() => setEditing(true)}><Pencil /></Button>
          <Button size="icon" variant="ghost" className="max-md:size-8 text-destructive hover:text-destructive" title="删除" aria-label={`删除 ${proxy.name}`} disabled={busy} onClick={() => void mutate(true)}><Trash2 /></Button>
        </div>
        {editing && <ExternalImportDialog editing={proxy} onClose={() => setEditing(false)} />}
      </TableCell>
    </TableRow>
  )
}

export function ExternalAuthorization({ userId, onState, saveButtonContainer }: {
  userId: number
  onState?: (state: { dirty: boolean; saving: boolean; count: number }) => void
  saveButtonContainer: HTMLElement | null
}) {
  const [baseline, setBaseline] = useState("[]")
  const { items, loading, error, reload } = useExternalProxies()
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [loaded, setLoaded] = useState(false)
  const [accessError, setAccessError] = useState("")
  const [saving, setSaving] = useState(false)
  const [query, setQuery] = useState("")
  const loadAccess = useCallback(() => {
    void api<{ ids: number[] }>(`/users/${userId}/external-proxies`).then(r => { setSelected(new Set(r.ids)); setBaseline(JSON.stringify([...r.ids].sort((a,b) => a-b))); setLoaded(true); setAccessError("") }).catch((e: Error) => setAccessError(e.message))
  }, [userId])
  useEffect(loadAccess, [loadAccess])
  useEffect(() => { onState?.({ dirty: JSON.stringify([...selected].sort((a,b) => a-b)) !== baseline, saving, count: selected.size }) }, [selected, baseline, saving, onState])
  async function save() {
    setSaving(true)
    try { await api(`/users/${userId}/external-proxies`, { method: "PUT", body: JSON.stringify({ ids: [...selected] }) }); setBaseline(JSON.stringify([...selected].sort((a,b) => a-b))); window.dispatchEvent(new Event("external-authorizations-changed")); toast.success("外部代理授权已保存") }
    catch (e) { toast.error((e as Error).message) }
    finally { setSaving(false) }
  }
  return <section className="mt-4 space-y-3 rounded-md border p-4" aria-label="外部代理授权"><h3 className="text-sm font-semibold">外部代理授权 · 独立保存</h3><p className="text-xs text-muted-foreground">仅进入 Mihomo 订阅，共用导入凭据，不采集统计。撤销授权不会让已复制的凭据失效。自动授权新增代理不包含外部导入。</p>
    {(error || accessError) && <p role="alert" className="text-sm text-destructive">{error || accessError}<Button type="button" variant="link" onClick={() => { void reload(); loadAccess() }}>重试</Button></p>}
    <Input aria-label="搜索外部代理授权" placeholder="搜索外部代理名称或地址" value={query} onChange={e => setQuery(e.target.value)} />
    <div className="flex gap-2"><Button type="button" variant="outline" disabled={!loaded || saving || loading} onClick={() => setSelected(new Set(items.map(p => p.id)))}>全选入口</Button><Button type="button" variant="outline" disabled={!loaded || saving || loading} onClick={() => setSelected(new Set())}>清空选择</Button></div>
    <div className="space-y-2">{items.filter(p => `${p.name} ${p.address}`.toLowerCase().includes(query.trim().toLowerCase())).map(p => <label key={p.id} className="flex items-center gap-3 text-sm"><input type="checkbox" checked={selected.has(p.id)} disabled={!loaded || saving || loading} onChange={e => setSelected(current => { const next = new Set(current); if (e.target.checked) next.add(p.id); else next.delete(p.id); return next })} /><span className="min-w-0 break-all">{p.name} · {p.protocol} · {p.address}:{p.port}{!p.enabled && "（已停用）"}</span></label>)}</div>
    {!loading && !items.length && !error && <p className="text-sm text-muted-foreground">尚无外部代理，请先在“代理”页面导入。</p>}
    <Button type="button" variant="ghost" disabled={saving || loading} onClick={loadAccess}>重新读取授权</Button>
    {saveButtonContainer && createPortal(
      <Button type="button" disabled={saving || !loaded || loading || !!error || !!accessError} onClick={() => void save()}>{saving ? "保存中…" : `保存外部授权（${selected.size}）`}</Button>,
      saveButtonContainer,
    )}
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
  const needle = query.trim().toLowerCase()
  const visible = items.filter(proxy =>
    `${proxy.name} ${proxy.protocol} ${proxy.address ?? ""}`.toLowerCase().includes(needle) &&
    (status === "all" || proxy.enabled === (status === "enabled")),
  )

  return (
    <div className="space-y-4">
      <div className="flex min-w-0 flex-wrap items-center gap-2 xl:flex-nowrap" aria-label="外部代理筛选与操作">
        <AdminSearchInput className="min-w-40 flex-1 basis-40" value={query} onChange={setQuery} placeholder="搜索名称、协议或地址" />
        <StatusFilter value={status} onChange={setStatus} labels={[["all", "全部状态"], ["enabled", "已启用"], ["disabled", "已停用"]]} />
        <div className="ml-auto flex shrink-0 items-center gap-2">
          <Button onClick={() => setImporting(true)}><Upload />导入外部代理</Button>
        </div>
      </div>
      <Card className="overflow-x-auto p-0">
        <Table className="min-w-max table-auto">
          <TableHeader className="bg-muted/50">
            <TableRow>
              <TableHead className="px-3">名称</TableHead>
              <TableHead className="px-3">类型</TableHead>
              <TableHead className="px-3">地址</TableHead>
              <TableHead className="px-3">端口</TableHead>
              <TableHead className="px-3 text-center">状态</TableHead>
              <TableHead className="px-3 text-right">操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {visible.map(proxy => <ExternalProxyRow key={proxy.id} proxy={proxy} />)}
            {!loading && !error && !visible.length && (
              <TableRow><TableCell colSpan={6} className="py-10 text-center text-sm text-muted-foreground">
                {items.length ? "没有匹配的外部代理" : "还没有外部代理，点击导入外部代理"}
              </TableCell></TableRow>
            )}
            {loading && <TableRow><TableCell colSpan={6} className="p-4"><Skeleton className="h-10 w-full" /></TableCell></TableRow>}
            {error && <TableRow><TableCell colSpan={6} className="py-8 text-center text-sm text-destructive">
              <div role="alert">加载外部代理失败：{error}</div>
              <Button variant="outline" size="sm" className="mt-3" onClick={() => void reload()}>重试</Button>
            </TableCell></TableRow>}
          </TableBody>
        </Table>
      </Card>
      {importing && <ExternalImportDialog onClose={() => setImporting(false)} />}
    </div>
  )
}
