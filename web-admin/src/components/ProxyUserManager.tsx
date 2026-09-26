import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Copy, LoaderCircle, Pencil, Plus, RefreshCw, RotateCw, Trash2 } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { api, type Node, type ProxyNode, type ProxyUser } from "@/lib/api"
import {
  createProxyUserOperationGuard,
  deleteProxyUser,
  proxyUserDeleteConfirmation,
  proxyUserRegenerateConfirmation,
  regenerateProxyUser,
  saveProxyUser,
  syncProxyUser,
  type ProxyUserInput,
  type ProxyUserResult,
} from "@/lib/proxy-user-actions"

type Confirmation = { kind: "regenerate" | "delete"; user: ProxyUser }
type FormState = ProxyUserInput

const NEW_USER: FormState = { name: "", enabled: true, note: "", proxy_node_ids: [] }

export function ProxyUserManager({ servers }: { servers: Node[] }) {
  const [users, setUsers] = useState<ProxyUser[]>([])
  const [proxyNodes, setProxyNodes] = useState<ProxyNode[]>([])
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [dialogUser, setDialogUser] = useState<ProxyUser | null | undefined>(undefined)
  const [confirm, setConfirm] = useState<Confirmation | null>(null)
  const [form, setForm] = useState<FormState>(NEW_USER)
  const [saving, setSaving] = useState(false)
  const [busyIds, setBusyIds] = useState<number[]>([])
  const [failedByUser, setFailedByUser] = useState<Record<number, ProxyUserResult["failed_servers"]>>({})
  const operationGuard = useRef(createProxyUserOperationGuard())
  const serverById = useMemo(() => new Map(servers.map((server) => [server.id, server])), [servers])

  const load = useCallback(async () => {
    try {
      const [userResult, nodeResult] = await Promise.all([
        api<{ users: ProxyUser[] }>("/proxy/users", { cache: "no-store" }),
        api<{ nodes: ProxyNode[] }>("/proxy/nodes", { cache: "no-store" }),
      ])
      setUsers(userResult.users)
      setProxyNodes(nodeResult.nodes)
      setFailedByUser({})
      setLoadError("")
    } catch (cause) {
      const message = (cause as Error).message
      setLoadError(message)
      toast.error(`用户数据加载失败：${message}`)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void Promise.resolve().then(load) }, [load])

  function openCreate() {
    setForm(NEW_USER)
    setDialogUser(null)
  }

  function openEdit(user: ProxyUser) {
    setForm({ name: user.name, enabled: user.enabled, note: user.note, proxy_node_ids: [...user.proxy_node_ids] })
    setDialogUser(user)
  }

  function replaceUser(user: ProxyUser) {
    setUsers((current) => current.map((entry) => entry.id === user.id ? user : entry))
  }

  function showSyncResult(user: ProxyUser, failed: ProxyUserResult["failed_servers"], successText: string) {
    replaceUser(user)
    setDialogUser((current) => current && current.id === user.id ? user : current)
    setFailedByUser((current) => ({ ...current, [user.id]: failed }))
    if (failed.length) {
      toast.error(`${successText}，同步失败：${failed.map((server) => server.server_name).join("、")}`)
    } else {
      toast.success(successText)
    }
  }

  async function submitForm(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!form.name.trim()) return toast.error("请填写用户名称")
    setSaving(true)
    try {
      const result = await saveProxyUser(api, dialogUser?.id ?? null, {
        name: form.name.trim(),
        enabled: form.enabled,
        note: form.note,
        proxy_node_ids: [...new Set(form.proxy_node_ids)],
      })
      if (dialogUser) {
        showSyncResult(result.user, result.failed_servers, "用户已保存")
      } else {
        setUsers((current) => [...current, result.user].sort((a, b) => a.created_at - b.created_at || a.id - b.id))
        setFailedByUser((current) => ({ ...current, [result.user.id]: result.failed_servers }))
        if (result.failed_servers.length) toast.error(`用户已创建，但 ${result.failed_servers.length} 台服务器同步失败`)
        else toast.success("用户已创建")
      }
      setDialogUser(undefined)
    } catch (cause) {
      toast.error(`保存用户失败：${(cause as Error).message}`)
      await load()
    } finally {
      setSaving(false)
    }
  }

  async function runUserOperation(user: ProxyUser, action: () => Promise<void>) {
    if (!operationGuard.current.tryStart(user.id)) return
    setBusyIds((current) => [...new Set([...current, user.id])])
    try {
      await action()
    } finally {
      operationGuard.current.finish(user.id)
      setBusyIds((current) => current.filter((id) => id !== user.id))
    }
  }

  async function toggleUser(user: ProxyUser, enabled: boolean) {
    await runUserOperation(user, async () => {
      try {
        const result = await saveProxyUser(api, user.id, {
          name: user.name,
          enabled,
          note: user.note,
          proxy_node_ids: user.proxy_node_ids,
        })
        showSyncResult(result.user, result.failed_servers, enabled ? "用户已启用" : "用户已停用")
      } catch (cause) {
        toast.error(`用户状态更新失败：${(cause as Error).message}`)
        await load()
      }
    })
  }

  async function confirmAction() {
    if (!confirm) return
    const target = confirm
    await runUserOperation(target.user, async () => {
      try {
        if (target.kind === "regenerate") {
          const result = await regenerateProxyUser(api, target.user.id)
          showSyncResult(result.user, result.failed_servers, "UUID 已重新生成")
        } else {
          const result = await deleteProxyUser(api, target.user.id)
          if (result.deleted) {
            setUsers((current) => current.filter((user) => user.id !== target.user.id))
            setFailedByUser((current) => {
              const next = { ...current }
              delete next[target.user.id]
              return next
            })
            toast.success("代理用户已删除")
          } else if (result.user) {
            replaceUser(result.user)
            setFailedByUser((current) => ({ ...current, [result.user!.id]: result.failed_servers }))
            toast.error(`用户已停用，但 ${result.failed_servers.length} 台服务器同步失败；记录已保留，可重试删除`)
          }
        }
      } catch (cause) {
        toast.error(`${target.kind === "delete" ? "删除" : "重新生成 UUID"}失败：${(cause as Error).message}`)
        await load()
      } finally {
        setConfirm(null)
      }
    })
  }

  async function resync(user: ProxyUser) {
    await runUserOperation(user, async () => {
      try {
        const result = await syncProxyUser(api, user.id)
        showSyncResult(result.user, result.failed_servers, "用户配置已重新同步")
      } catch (cause) {
        toast.error(`同步用户配置失败：${(cause as Error).message}`)
      }
    })
  }

  async function copyUuid(uuid: string) {
    try {
      if (!navigator.clipboard?.writeText) throw new Error("当前浏览器不支持剪贴板访问")
      await navigator.clipboard.writeText(uuid)
      toast.success("UUID 已复制")
    } catch {
      toast.error("复制失败")
    }
  }

  function toggleProxyNode(proxyNodeId: number, checked: boolean) {
    setForm((current) => ({
      ...current,
      proxy_node_ids: checked
        ? [...new Set([...current.proxy_node_ids, proxyNodeId])]
        : current.proxy_node_ids.filter((id) => id !== proxyNodeId),
    }))
  }

  const groupedNodes = useMemo(() => {
    const groups = new Map<number, ProxyNode[]>()
    for (const proxyNode of proxyNodes) {
      groups.set(proxyNode.node_id, [...(groups.get(proxyNode.node_id) ?? []), proxyNode])
    }
    return [...groups.entries()].sort(([a], [b]) => (serverById.get(a)?.name ?? "").localeCompare(serverById.get(b)?.name ?? ""))
  }, [proxyNodes, serverById])
  const editBusy = dialogUser != null && busyIds.includes(dialogUser.id)

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold">用户管理</h1>
          <p className="text-sm text-muted-foreground">管理代理用户及其可访问的代理节点。</p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline" onClick={() => { setLoading(true); void load() }} disabled={loading}>
            <RefreshCw className={`size-4 ${loading ? "animate-spin" : ""}`} /> 刷新
          </Button>
          <Button onClick={openCreate}><Plus className="size-4" /> 新建用户</Button>
        </div>
      </div>

      {loadError && <p role="alert" className="text-sm text-destructive">{loadError}</p>}
      <Card className="overflow-hidden p-0">
        {loading && !users.length ? (
          <div className="flex items-center gap-2 p-6 text-sm text-muted-foreground"><LoaderCircle className="size-4 animate-spin" />正在加载用户…</div>
        ) : !users.length ? (
          <div className="p-8 text-center text-sm text-muted-foreground">还没有代理用户。</div>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>用户</TableHead>
                <TableHead>UUID</TableHead>
                <TableHead>状态</TableHead>
                <TableHead>代理节点</TableHead>
                <TableHead>备注</TableHead>
                <TableHead className="text-right">操作</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {users.map((user) => {
                const busy = busyIds.includes(user.id)
                const nodeIssues = user.proxy_node_ids
                  .map((id) => proxyNodes.find((node) => node.id === id))
                  .filter((node): node is ProxyNode => !!node && node.deploy_status !== "deployed")
                const inferredFailures = [...new Map(nodeIssues.map((node) => [node.node_id, {
                  server_id: node.node_id,
                  server_name: serverById.get(node.node_id)?.name ?? `服务器 ${node.node_id}`,
                  error: node.last_error || (node.deploy_status === "deploying" ? "配置正在部署" : "代理节点尚未成功部署"),
                }])).values()]
                const failures = failedByUser[user.id] ?? inferredFailures
                return (
                  <TableRow key={user.id}>
                    <TableCell className="font-medium">
                      {user.name}
                      {user.is_system && <span className="ml-2 rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">系统</span>}
                    </TableCell>
                    <TableCell>
                      <div className="flex items-center gap-1.5 font-mono text-xs">
                        <span title={user.uuid}>{user.uuid.slice(0, 8)}…{user.uuid.slice(-4)}</span>
                        <Button size="icon" variant="ghost" className="size-7" title="复制 UUID" onClick={() => void copyUuid(user.uuid)}>
                          <Copy className="size-3.5" />
                        </Button>
                      </div>
                    </TableCell>
                    <TableCell>
                      <div className="flex items-center gap-2">
                        <Switch checked={user.enabled} disabled={busy} onCheckedChange={(enabled) => void toggleUser(user, enabled)} aria-label={`${user.enabled ? "停用" : "启用"} ${user.name}`} />
                        <span className="text-xs text-muted-foreground">{user.enabled ? "启用" : "停用"}</span>
                      </div>
                      {failures.length > 0 && <span className="mt-1 block text-xs text-destructive" title={failures.map((failure) => `${failure.server_name}：${failure.error}`).join("\n")}>服务器配置未同步</span>}
                    </TableCell>
                    <TableCell className="max-w-64">
                      <div className="flex flex-wrap gap-1">
                        {user.proxy_node_ids.length ? user.proxy_node_ids.map((id) => {
                          const proxyNode = proxyNodes.find((node) => node.id === id)
                          const serverName = proxyNode ? serverById.get(proxyNode.node_id)?.name ?? `服务器 ${proxyNode.node_id}` : "已删除节点"
                          return <span key={id} className="rounded bg-muted px-1.5 py-0.5 text-xs" title={`${serverName} · VLESS + Reality · ${proxyNode?.listen_port ?? "-"}`}>{proxyNode?.name ?? `节点 ${id}`}</span>
                        }) : <span className="text-xs text-muted-foreground">未分配</span>}
                      </div>
                    </TableCell>
                    <TableCell className="max-w-48 truncate text-sm text-muted-foreground" title={user.note}>{user.note || "-"}</TableCell>
                    <TableCell>
                      <div className="flex justify-end gap-1">
                        <Button size="sm" variant="ghost" disabled={busy} onClick={() => openEdit(user)}><Pencil className="size-4" />编辑</Button>
                        <Button size="icon" variant="ghost" disabled={busy} title="重新同步" onClick={() => void resync(user)}><RotateCw className="size-4" /></Button>
                        <Button size="icon" variant="ghost" disabled={busy} title="重新生成 UUID" onClick={() => setConfirm({ kind: "regenerate", user })}><RefreshCw className="size-4" /></Button>
                        {!user.is_system && <Button size="icon" variant="ghost" disabled={busy} title="删除用户" onClick={() => setConfirm({ kind: "delete", user })}><Trash2 className="size-4 text-destructive" /></Button>}
                      </div>
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        )}
      </Card>

      <Dialog open={dialogUser !== undefined} onOpenChange={(open) => !open && !saving && setDialogUser(undefined)}>
        <DialogContent className="max-h-[calc(100dvh-64px)] overflow-y-auto sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>{dialogUser ? "编辑代理用户" : "新建代理用户"}</DialogTitle>
            <DialogDescription>用户 UUID 由服务器生成；保存后会同步到授权节点。</DialogDescription>
          </DialogHeader>
          <form onSubmit={(event) => void submitForm(event)} className="space-y-5">
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="proxy-user-name">用户名称</Label>
                <Input id="proxy-user-name" value={form.name} maxLength={120} autoFocus disabled={saving || editBusy || !!dialogUser?.is_system} onChange={(event) => setForm((current) => ({ ...current, name: event.target.value }))} />
              </div>
              <div className="flex items-end justify-between rounded-md border px-3 py-2.5">
                <div><Label htmlFor="proxy-user-enabled">状态</Label><p className="text-xs text-muted-foreground">停用后会从授权节点配置中移除。</p></div>
                <Switch id="proxy-user-enabled" checked={form.enabled} disabled={saving || editBusy} onCheckedChange={(enabled) => setForm((current) => ({ ...current, enabled }))} />
              </div>
            </div>

            {dialogUser ? (
              <div className="space-y-2">
                <Label>用户 UUID</Label>
                <div className="flex gap-2">
                  <Input readOnly value={dialogUser.uuid} className="font-mono" />
                  <Button type="button" variant="outline" onClick={() => void copyUuid(dialogUser.uuid)}><Copy className="size-4" />复制</Button>
                  <Button type="button" variant="outline" onClick={() => setConfirm({ kind: "regenerate", user: dialogUser })}>重新生成</Button>
                </div>
              </div>
            ) : (
              <p className="rounded-md bg-muted px-3 py-2 text-sm text-muted-foreground">UUID：创建时自动生成。</p>
            )}

            <fieldset className="space-y-2">
              <legend className="text-sm font-medium">可用代理节点</legend>
              {groupedNodes.length ? (
                <div className="max-h-64 space-y-3 overflow-y-auto rounded-md border p-3">
                  {groupedNodes.map(([serverId, group]) => (
                    <section key={serverId} className="space-y-1.5">
                      <h3 className="text-xs font-medium text-muted-foreground">{serverById.get(serverId)?.name ?? `服务器 ${serverId}`}</h3>
                      {group.map((proxyNode) => (
                        <label key={proxyNode.id} className="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 hover:bg-muted">
                          <input
                            type="checkbox"
                            checked={form.proxy_node_ids.includes(proxyNode.id)}
                            disabled={saving || editBusy}
                            onChange={(event) => toggleProxyNode(proxyNode.id, event.target.checked)}
                            className="size-4 accent-primary"
                          />
                          <span className="min-w-0 flex-1 text-sm">{proxyNode.name}<span className="ml-2 text-xs text-muted-foreground">VLESS + Reality · {proxyNode.listen_port}</span></span>
                          {!proxyNode.enabled && <span className="text-xs text-muted-foreground">节点已停用</span>}
                        </label>
                      ))}
                    </section>
                  ))}
                </div>
              ) : <p className="rounded-md border border-dashed p-4 text-sm text-muted-foreground">当前没有可分配的代理节点。</p>}
            </fieldset>

            <div className="space-y-2">
              <Label htmlFor="proxy-user-note">备注</Label>
              <textarea
                id="proxy-user-note"
                value={form.note}
                maxLength={2000}
                rows={3}
                disabled={saving || editBusy}
                onChange={(event) => setForm((current) => ({ ...current, note: event.target.value }))}
                className="flex w-full resize-y rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-xs outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50"
              />
            </div>
            <DialogFooter>
              <Button type="button" variant="outline" disabled={saving || editBusy} onClick={() => setDialogUser(undefined)}>取消</Button>
              <Button type="submit" disabled={saving || editBusy}>
                {saving && <LoaderCircle className="size-4 animate-spin" />}
                {dialogUser ? "保存并同步" : "创建并同步"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={confirm !== null} onOpenChange={(open) => !open && setConfirm(null)}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{confirm ? (confirm.kind === "delete" ? proxyUserDeleteConfirmation(confirm.user.name).title : proxyUserRegenerateConfirmation(confirm.user.name).title) : "确认操作"}</DialogTitle>
            <DialogDescription>
              {confirm ? (confirm.kind === "delete" ? proxyUserDeleteConfirmation(confirm.user.name).description : proxyUserRegenerateConfirmation(confirm.user.name).description) : ""}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" disabled={confirm ? busyIds.includes(confirm.user.id) : false} onClick={() => setConfirm(null)}>取消</Button>
            <Button variant={confirm?.kind === "delete" ? "destructive" : "default"} disabled={confirm ? busyIds.includes(confirm.user.id) : false} onClick={() => void confirmAction()}>
              {confirm?.kind === "delete" ? "删除用户" : "重新生成 UUID"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
