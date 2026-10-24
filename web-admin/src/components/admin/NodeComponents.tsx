import { useEffect, useState } from "react"
import { toast } from "sonner"
import { Button } from "@/components/ui/button"
import { Switch } from "@/components/ui/switch"
import { Badge } from "@/components/ui/badge"
import { getComponents, saveComponents, sameInstall, type InstallChoices, type ComponentProfile } from "@/lib/node-components"
import { OptionRow } from "./Shared"
const labels = { singbox: "sing-box", realm: "Realm", mieru: "mieru" }
export function NodeComponents({ nodeId, onReady }: { nodeId: number; onReady: (ready: boolean) => void }) {
  const [storedProfile, setProfile] = useState<ComponentProfile | null>(null)
  const [loadedNodeId, setLoadedNodeId] = useState<number | null>(null)
  const profile = loadedNodeId === nodeId ? storedProfile : null
  const [choices, setChoices] = useState<InstallChoices>({ singbox: true, realm: false, mieru: false })
  const [error, setError] = useState("")
  const [busy, setBusy] = useState(false)
  const [reload, setReload] = useState(0)
  const dirty = profile !== null && !sameInstall(choices, profile.install)
  useEffect(() => {
    onReady(false)
    const controller = new AbortController()
    void getComponents(nodeId, controller.signal).then((value) => {
      if (!controller.signal.aborted) { setProfile(value); setLoadedNodeId(nodeId); setChoices(value.install); setError("") }
    }).catch((e: Error) => { if (!controller.signal.aborted) setError(e.message) })
    return () => controller.abort()
  }, [nodeId, reload, onReady])
  useEffect(() => { onReady(profile !== null && !dirty && !busy && !error) }, [profile, dirty, busy, error, onReady])
  function change(key: "realm" | "mieru", enabled: boolean) { onReady(false); setChoices((previous) => ({ ...previous, [key]: enabled })) }
  async function save() {
    if (!profile || busy) return
    const removing = (["realm", "mieru"] as const).filter((key) => !choices[key] && (profile.install[key] || profile.components.some((c) => c.component === key && c.installed)))
    if (removing.length && !confirm(`执行新的安装命令后，将停止并卸载 ${removing.map((key) => labels[key]).join("、")}，配置及统计文件保留。现在保存安装选项？`)) return
    setBusy(true); onReady(false)
    try { const result = await saveComponents(nodeId, choices); setProfile(result); setChoices(result.install); setError(""); toast.success("安装选项已保存，执行命令后生效") }
    catch (e) { setError((e as Error).message) } finally { setBusy(false) }
  }
  const lastChecked = profile ? Math.max(0, ...profile.components.map((c) => c.checked_at ?? 0)) : 0
  return <div className="space-y-3">
    <OptionRow title="sing-box" hint="每次安装和升级均安装"><Badge variant="secondary" className="whitespace-nowrap">必装</Badge></OptionRow>
    {(["realm", "mieru"] as const).map((key) => <OptionRow key={key} title={`安装 ${labels[key]}`} hint="关闭后卸载托管组件，配置保留"><Switch aria-label={`安装 ${labels[key]}`} checked={choices[key]} disabled={!profile || busy} onCheckedChange={(on) => change(key, on)} /></OptionRow>)}
    {error && <div role="alert" className="flex min-w-0 items-center gap-2 text-sm text-destructive"><span className="min-w-0 truncate" title={error}>{error}</span><Button size="sm" variant="outline" disabled={busy} onClick={() => { setProfile(null); setError(""); setReload((n) => n + 1) }}>重新读取</Button></div>}
    {!profile && !error && <p className="text-xs text-muted-foreground">正在读取安装选项…</p>}
    <div className="flex items-center justify-between gap-2"><span className="text-xs text-muted-foreground">{dirty ? "选项尚未保存" : profile && !error ? "已保存的安装选项" : ""}</span><Button type="button" size="sm" variant="outline" disabled={!profile || !dirty || busy} onClick={() => void save()}>{busy ? "保存中…" : "保存安装选项"}</Button></div>
    {profile && <section className="space-y-2 rounded-md border p-3"><p className="text-xs font-medium">{profile.online ? "组件最近上报状态" : "组件最近上报状态 · 节点离线"}</p>{profile.components.map((c) => <div key={c.component} className="grid min-w-0 grid-cols-[5rem_1fr_auto] items-center gap-2 text-xs">
      <span className="whitespace-nowrap font-medium">{labels[c.component]}</span>
      <span className={`min-w-0 truncate ${c.last_error ? "text-destructive" : "text-muted-foreground"}`} title={c.last_error || [c.version, c.managed ? "安装器管理" : "非托管", c.checked_at ? new Date(c.checked_at * 1000).toLocaleString() : "未上报"].join(" · ")}>{c.last_error || (c.checked_at === null ? "未上报" : `${c.installed ? "已安装" : "未安装"}${c.installed ? c.managed ? " · 托管" : " · 非托管" : ""}`)}</span>
      <span className="whitespace-nowrap text-muted-foreground">{c.checked_at === null ? "—" : c.running ? "运行中" : "已停止"}</span>
    </div>)}<p className="truncate text-xs text-muted-foreground" title={lastChecked ? new Date(lastChecked * 1000).toLocaleString() : undefined}>{lastChecked ? `最近上报：${new Date(lastChecked * 1000).toLocaleString()}` : "等待 Agent 首次上报"}</p></section>}
  </div>
}
