import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { ChevronDown, Copy, ExternalLink, KeyRound, LoaderCircle, Pencil, Plus, RefreshCw, RotateCcw, RotateCw, Search, Trash2 } from "lucide-react"
import { toast } from "sonner"

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Progress } from "@/components/ui/progress"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Skeleton } from "@/components/ui/skeleton"
import { Switch } from "@/components/ui/switch"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip"
import { api, type Node, type ProxyNode, type ProxyUser } from "@/lib/api"
import {
  createProxyUserOperationGuard,
  deleteProxyUser,
  proxyUserDeleteConfirmation,
  proxyUserRegenerateConfirmation,
  regenerateProxyUser,
  resetProxyUserTraffic,
  saveProxyUser,
  setProxyUserPassword,
  syncProxyUser,
  type ProxyUserInput,
  type ProxyUserResult,
} from "@/lib/proxy-user-actions"
import {
  expiryDateLabel,
  filterProxyUsers,
  formatBytes,
  limitBytesFromGb,
  limitGbInput,
  proxyUserCountText,
  proxyUserListState,
  proxyUserRowView,
  PROXY_USER_TABLE_COLUMNS,
  trafficPercent,
  type ProxyUserStatusFilter,
} from "@/lib/proxy-user-view"

type Confirmation = { kind: "regenerate" | "delete" | "traffic"; user: ProxyUser }
type FormState = {
  name: string
  enabled: boolean
  note: string
  proxy_node_ids: number[]
  traffic_limit_gb: string
  traffic_reset_day: string
  expire_date: string
  password: string
}

const NEW_USER: FormState = {
  name: "",
  enabled: true,
  note: "",
  proxy_node_ids: [],
  traffic_limit_gb: "0",
  traffic_reset_day: "1",
  expire_date: "",
  password: "",
}

function formFromUser(user: ProxyUser): FormState {
  return {
    name: user.name,
    enabled: user.enabled,
    note: user.note,
    proxy_node_ids: [...user.proxy_node_ids],
    traffic_limit_gb: limitGbInput(user.traffic_limit_bytes),
    traffic_reset_day: String(user.traffic_reset_day),
    expire_date: user.expire_date ?? "",
    password: "",
  }
}

function accessLabel(user: ProxyUser): string {
  switch (user.access_state) {
    case "admin_disabled": return "停用"
    case "expired": return "已到期"
    case "traffic_exceeded": return "已超额"
    default: return "启用"
  }
}

function accessClass(user: ProxyUser): string {
  switch (user.access_state) {
    case "enabled": return "border-emerald-600/20 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400"
    case "expired":
    case "traffic_exceeded": return "border-amber-600/20 bg-amber-600/10 text-amber-700 dark:text-amber-400"
    default: return "border-border bg-muted text-muted-foreground"
  }
}

export function ProxyUserManager({ servers }: { servers: Node[] }) {
  const [users, setUsers] = useState<ProxyUser[]>([])
  const [proxyNodes, setProxyNodes] = useState<ProxyNode[]>([])
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState("")
  const [query, setQuery] = useState("")
  const [statusFilter, setStatusFilter] = useState<ProxyUserStatusFilter>("all")
  const [dialogUser, setDialogUser] = useState<ProxyUser | null | undefined>(undefined)
  const [passwordUser, setPasswordUser] = useState<ProxyUser | null>(null)
  const [passwordValue, setPasswordValue] = useState("")
  const [passwordSaving, setPasswordSaving] = useState(false)
  const [confirm, setConfirm] = useState<Confirmation | null>(null)
  const [form, setForm] = useState<FormState>(NEW_USER)
  const [advancedOpen, setAdvancedOpen] = useState(false)
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
    setAdvancedOpen(true)
    setDialogUser(null)
  }

  function openEdit(user: ProxyUser) {
    setForm(formFromUser(user))
    setAdvancedOpen(false)
    setDialogUser(user)
  }

  function replaceUser(user: ProxyUser) {
    setUsers((current) => current.map((entry) => entry.id === user.id ? user : entry))
  }

  function showSyncResult(user: ProxyUser, failed: ProxyUserResult["failed_servers"], successText: string) {
    replaceUser(user)
    setDialogUser((current) => current && current.id === user.id ? user : current)
    setFailedByUser((current) => ({ ...current, [user.id]: failed }))
    if (failed.length) toast.error(`${successText}，同步失败：${failed.map((server) => server.server_name).join("、")}`)
    else toast.success(successText)
  }

  async function submitForm(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!form.name.trim()) return toast.error("请填写用户名")
    const passwordBytes = new TextEncoder().encode(form.password).length
    if (!dialogUser && passwordBytes < 12) return toast.error("用户登录密码至少 12 字节")
    if (passwordBytes > 1024) return toast.error("用户登录密码不能超过 1024 字节")
    const unchangedLimit = dialogUser !== null && dialogUser !== undefined
      && form.traffic_limit_gb === limitGbInput(dialogUser.traffic_limit_bytes)
    const limitBytes = unchangedLimit ? dialogUser.traffic_limit_bytes : limitBytesFromGb(form.traffic_limit_gb)
    if (limitBytes === null) return toast.error("流量限额必须是有效的非负 GB 数值")
    const resetDay = Number(form.traffic_reset_day)
    if (!Number.isInteger(resetDay) || resetDay < 1 || resetDay > 28) return toast.error("每月重置日必须在 1 到 28 之间")
    setSaving(true)
    try {
      const input: ProxyUserInput = {
        name: form.name.trim(),
        enabled: form.enabled,
        note: form.note,
        proxy_node_ids: [...new Set(form.proxy_node_ids)],
        traffic_limit_bytes: limitBytes,
        traffic_reset_day: resetDay,
        expire_date: dialogUser !== null && dialogUser !== undefined
          && form.expire_date === (dialogUser.expire_date ?? "")
          ? undefined
          : form.expire_date || null,
        password: dialogUser ? undefined : form.password,
      }
      const result = await saveProxyUser(api, dialogUser?.id ?? null, input)
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

  async function submitPassword(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!passwordUser) return
    const passwordBytes = new TextEncoder().encode(passwordValue).length
    if (passwordBytes < 12 || passwordBytes > 1024) {
      return toast.error("密码长度须为 12 到 1024 字节")
    }
    setPasswordSaving(true)
    try {
      await setProxyUserPassword(api, passwordUser.id, passwordValue)
      toast.success("用户中心密码已更新，该用户的现有用户中心会话已退出")
      setPasswordUser(null)
      setPasswordValue("")
    } catch (cause) {
      toast.error(`更新登录密码失败：${(cause as Error).message}`)
    } finally {
      setPasswordSaving(false)
    }
  }

  function openPassword(user: ProxyUser) {
    setPasswordValue("")
    setPasswordUser(user)
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
          traffic_limit_bytes: user.traffic_limit_bytes,
          traffic_reset_day: user.traffic_reset_day,
        })
        showSyncResult(result.user, result.failed_servers, enabled ? "用户已启用" : "用户已停用")
      } catch (cause) {
        toast.error(`用户状态更新失败：${(cause as Error).message}`)
        await load()
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

  async function confirmAction() {
    if (!confirm) return
    const target = confirm
    await runUserOperation(target.user, async () => {
      try {
        if (target.kind === "regenerate") {
          const result = await regenerateProxyUser(api, target.user.id)
          showSyncResult(result.user, result.failed_servers, "UUID 已重新生成")
        } else if (target.kind === "traffic") {
          const result = await resetProxyUserTraffic(api, target.user.id)
          showSyncResult(result.user, result.failed_servers, "流量已清空")
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
        const label = target.kind === "delete" ? "删除" : target.kind === "traffic" ? "清空流量" : "重新生成 UUID"
        toast.error(`${label}失败：${(cause as Error).message}`)
        await load()
      } finally {
        setConfirm(null)
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
    for (const proxyNode of proxyNodes) groups.set(proxyNode.node_id, [...(groups.get(proxyNode.node_id) ?? []), proxyNode])
    return [...groups.entries()].sort(([a], [b]) => (serverById.get(a)?.name ?? "").localeCompare(serverById.get(b)?.name ?? ""))
  }, [proxyNodes, serverById])
  const editBusy = dialogUser != null && busyIds.includes(dialogUser.id)
  const listState = proxyUserListState(users.length, loading)
  const filteredUsers = filterProxyUsers(users, query, statusFilter)

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-end gap-2">
        <div className="mr-auto flex w-full items-center gap-2 sm:w-auto">
          <div className="relative min-w-0 flex-1 sm:w-64 sm:flex-none">
            <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input className="pl-8" placeholder="搜索用户名" aria-label="按用户名搜索" value={query} onChange={(event) => setQuery(event.target.value)} />
          </div>
          <Select value={statusFilter} onValueChange={(value) => setStatusFilter(value as ProxyUserStatusFilter)}>
            <SelectTrigger className="w-32" aria-label="按状态筛选"><SelectValue /></SelectTrigger>
            <SelectContent position="popper">
              <SelectItem value="all">全部状态</SelectItem>
              <SelectItem value="enabled">启用</SelectItem>
              <SelectItem value="disabled">停用</SelectItem>
            </SelectContent>
          </Select>
          <span className="hidden whitespace-nowrap text-xs text-muted-foreground sm:inline">{proxyUserCountText(users.length, listState === "loading")}</span>
        </div>
        <Button variant="outline" onClick={() => { setLoading(true); void load() }} disabled={loading}>
          <RefreshCw className={loading ? "animate-spin" : ""} /> 刷新
        </Button>
        <Button onClick={openCreate}><Plus /> 新建用户</Button>
      </div>

      {loadError && <p role="alert" className="text-sm text-destructive">{loadError}</p>}
      <TooltipProvider delayDuration={250}>
        <Card className="overflow-x-auto p-0">
          <Table>
            <TableHeader>
              <TableRow>
                {PROXY_USER_TABLE_COLUMNS.map((column) => <TableHead key={column.key} className={column.className}>{column.label}</TableHead>)}
              </TableRow>
            </TableHeader>
            <TableBody>
              {listState === "loading" ? Array.from({ length: 3 }, (_, index) => (
                <TableRow key={`loading-${index}`}>
                  {PROXY_USER_TABLE_COLUMNS.map((column) => <TableCell key={column.key}><Skeleton className="h-4 w-24" /></TableCell>)}
                </TableRow>
              )) : listState === "empty" ? (
                <TableRow><TableCell colSpan={PROXY_USER_TABLE_COLUMNS.length} className="h-28 text-center text-sm text-muted-foreground">暂无用户</TableCell></TableRow>
              ) : filteredUsers.length === 0 ? (
                <TableRow><TableCell colSpan={PROXY_USER_TABLE_COLUMNS.length} className="h-28 text-center text-sm text-muted-foreground">没有匹配的用户</TableCell></TableRow>
              ) : filteredUsers.map((user) => {
                const busy = busyIds.includes(user.id)
                const row = proxyUserRowView(user)
                const percent = trafficPercent(user.traffic.used_bytes, user.traffic_limit_bytes)
                const progressColor = percent >= 100 ? "bg-destructive" : percent >= 80 ? "bg-amber-500" : "bg-primary"
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
                    <TableCell className="font-mono text-sm text-muted-foreground">{row.id}</TableCell>
                    <TableCell className="font-medium">
                      {user.name}
                      {row.systemLabel && <Badge variant="secondary" className="ml-2">{row.systemLabel}</Badge>}
                    </TableCell>
                    <TableCell>
                      <div className="space-y-2">
                        <div className="flex min-w-0 flex-nowrap items-center gap-1.5">
                          <span className="shrink-0 whitespace-nowrap font-medium tabular-nums">{formatBytes(user.traffic.used_bytes)}</span>
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <Button type="button" size="xs" variant="ghost" className="h-5 px-1.5 text-xs font-normal text-muted-foreground hover:bg-transparent hover:text-foreground" disabled={busy} onClick={() => setConfirm({ kind: "traffic", user })}>
                                <RotateCcw className="size-3" /> 清空
                              </Button>
                            </TooltipTrigger>
                            <TooltipContent>清空 Hub 当前周期累计，并在服务器下次上报时重新建立 baseline</TooltipContent>
                          </Tooltip>
                          <span className="ml-auto min-w-0 truncate whitespace-nowrap text-right text-xs text-muted-foreground">{user.traffic_limit_bytes === 0 ? "不限量" : `${formatBytes(user.traffic_limit_bytes)} (${Math.floor(percent)}%)`}</span>
                        </div>
                        <Progress className="h-1.5" aria-label={`${user.name} 流量使用比例`} value={percent} indicatorClassName={progressColor} />
                      </div>
                    </TableCell>
                    <TableCell className="text-sm">{expiryDateLabel(user.expire_date)}</TableCell>
                    <TableCell>
                      <div className="flex items-center gap-2">
                        <Switch checked={user.enabled} disabled={busy} onCheckedChange={(enabled) => void toggleUser(user, enabled)} aria-label={`${user.enabled ? "停用" : "启用"} ${user.name}`} />
                        <Badge variant="outline" className={accessClass(user)}>{accessLabel(user)}</Badge>
                      </div>
                      {failures.length > 0 && <span className="mt-1 block text-xs text-destructive" title={failures.map((failure) => `${failure.server_name}：${failure.error}`).join("\n")}>服务器配置未同步</span>}
                    </TableCell>
                    <TableCell>
                      <div className="flex justify-end gap-1">
                        <Button type="button" size="icon-sm" variant="ghost" disabled={busy} title="以用户身份查看" aria-label={`以用户身份查看 ${user.name}`} onClick={() => { window.open("/user?preview=1", "_blank", "noopener,noreferrer") }}><ExternalLink className="size-4" /></Button>
                        {row.actions.includes("edit") && <Button size="sm" variant="ghost" disabled={busy} onClick={() => openEdit(user)}><Pencil className="size-4" /> 编辑</Button>}
                        {!user.is_system && <Button size="icon-sm" variant="ghost" disabled={busy} title="设置用户中心密码" aria-label={`设置 ${user.name} 的用户中心密码`} onClick={() => openPassword(user)}><KeyRound className="size-4" /></Button>}
                        {row.actions.includes("delete") && <Button size="icon-sm" variant="ghost" disabled={busy} title="删除用户" onClick={() => setConfirm({ kind: "delete", user })}><Trash2 className="size-4 text-destructive" /></Button>}
                      </div>
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        </Card>
      </TooltipProvider>

      <Dialog open={dialogUser !== undefined} onOpenChange={(open) => !open && !saving && setDialogUser(undefined)}>
        <DialogContent className="max-h-[calc(100dvh-32px)] overflow-y-auto sm:max-w-[560px]">
          <DialogHeader>
            <DialogTitle>{dialogUser ? `编辑用户：${dialogUser.name}` : "新建代理用户"}</DialogTitle>
            <DialogDescription>修改该用户的流量额度、重置日期与有效期。</DialogDescription>
          </DialogHeader>
          <form onSubmit={(event) => void submitForm(event)} className="space-y-5">
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="proxy-user-limit">流量限额 (GB)</Label>
                <Input id="proxy-user-limit" type="number" inputMode="decimal" min="0" step="0.01" value={form.traffic_limit_gb} disabled={saving || editBusy} onChange={(event) => setForm((current) => ({ ...current, traffic_limit_gb: event.target.value }))} />
                <p className="text-xs text-muted-foreground">0 表示不限量，按 1 GB = 1024³ bytes 保存。</p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="proxy-user-reset-day">每月重置日</Label>
                <Select value={form.traffic_reset_day} disabled={saving || editBusy} onValueChange={(value) => setForm((current) => ({ ...current, traffic_reset_day: value }))}>
                  <SelectTrigger id="proxy-user-reset-day"><SelectValue /></SelectTrigger>
                  <SelectContent>{Array.from({ length: 28 }, (_, index) => String(index + 1)).map((day) => <SelectItem key={day} value={day}>每月 {day} 日</SelectItem>)}</SelectContent>
                </Select>
              </div>
            </div>
            <div className="space-y-2">
              <Label htmlFor="proxy-user-expiry">到期时间（留空为永久有效）</Label>
              <Input id="proxy-user-expiry" type="date" value={form.expire_date} disabled={saving || editBusy} onChange={(event) => setForm((current) => ({ ...current, expire_date: event.target.value }))} />
              <p className="text-xs text-muted-foreground">所选日期全天有效，Hub 时区次日 00:00 到期。</p>
            </div>

            <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen} className="rounded-lg border">
              <CollapsibleTrigger asChild>
                <Button type="button" variant="ghost" className="h-10 w-full justify-between px-3 font-medium">
                  代理设置 <ChevronDown className={`size-4 transition-transform ${advancedOpen ? "rotate-180" : ""}`} />
                </Button>
              </CollapsibleTrigger>
              <CollapsibleContent className="space-y-4 border-t p-4">
                <div className="grid gap-4 sm:grid-cols-2">
                  <div className="space-y-2">
                    <Label htmlFor="proxy-user-name">用户名</Label>
                    <Input id="proxy-user-name" value={form.name} maxLength={120} autoFocus disabled={saving || editBusy || !!dialogUser?.is_system} onChange={(event) => setForm((current) => ({ ...current, name: event.target.value }))} />
                  </div>
                  <div className="flex items-center justify-between rounded-md border px-3 py-2.5">
                    <div><Label htmlFor="proxy-user-enabled">管理员开关</Label><p className="text-xs text-muted-foreground">关闭后会从节点配置中移除。</p></div>
                    <Switch id="proxy-user-enabled" checked={form.enabled} disabled={saving || editBusy} onCheckedChange={(enabled) => setForm((current) => ({ ...current, enabled }))} />
                  </div>
                </div>
                {!dialogUser && <div className="space-y-2">
                  <Label htmlFor="proxy-user-password">用户中心登录密码</Label>
                  <Input id="proxy-user-password" type="password" autoComplete="new-password" maxLength={1024} required value={form.password} disabled={saving} onChange={(event) => setForm((current) => ({ ...current, password: event.target.value }))} />
                  <p className="text-xs text-muted-foreground">至少 12 字节；服务器只保存 Argon2 密码 hash。</p>
                </div>}
                {dialogUser ? (
                  <div className="space-y-2">
                    <Label>用户 UUID</Label>
                    <div className="flex flex-wrap gap-2">
                      <Input readOnly value={dialogUser.uuid} className="min-w-48 flex-1 font-mono" />
                      <Button type="button" variant="outline" onClick={() => void copyUuid(dialogUser.uuid)}><Copy className="size-4" />复制</Button>
                      <Button type="button" variant="outline" onClick={() => setConfirm({ kind: "regenerate", user: dialogUser })}>重新生成</Button>
                    </div>
                  </div>
                ) : <p className="rounded-md bg-muted px-3 py-2 text-sm text-muted-foreground">UUID 将在服务器创建时生成。</p>}
                {dialogUser && !dialogUser.is_system && <div className="flex items-center justify-between gap-3 rounded-md border px-3 py-2.5">
                  <div><p className="text-sm font-medium">用户中心密码</p><p className="text-xs text-muted-foreground">密码不会回显，重设后会撤销该用户现有会话。</p></div>
                  <Button type="button" variant="outline" disabled={saving || editBusy} onClick={() => openPassword(dialogUser)}>重置密码</Button>
                </div>}

                <fieldset className="space-y-2">
                  <legend className="text-sm font-medium">可用代理节点</legend>
                  {groupedNodes.length ? (
                    <div className="max-h-48 space-y-3 overflow-y-auto rounded-md border p-3">
                      {groupedNodes.map(([serverId, group]) => (
                        <section key={serverId} className="space-y-1.5">
                          <h3 className="text-xs font-medium text-muted-foreground">{serverById.get(serverId)?.name ?? `服务器 ${serverId}`}</h3>
                          {group.map((proxyNode) => (
                            <label key={proxyNode.id} className="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 hover:bg-muted">
                              <input type="checkbox" checked={form.proxy_node_ids.includes(proxyNode.id)} disabled={saving || editBusy} onChange={(event) => toggleProxyNode(proxyNode.id, event.target.checked)} className="size-4 accent-primary" />
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
                  <textarea id="proxy-user-note" value={form.note} maxLength={2000} rows={2} disabled={saving || editBusy} onChange={(event) => setForm((current) => ({ ...current, note: event.target.value }))} className="flex w-full resize-y rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-xs outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50" />
                </div>
              </CollapsibleContent>
            </Collapsible>

            <DialogFooter>
              <Button type="button" variant="outline" disabled={saving || editBusy} onClick={() => setDialogUser(undefined)}>取消</Button>
              {dialogUser && <Button type="button" variant="outline" disabled={saving || editBusy} onClick={() => void resync(dialogUser)}><RotateCw className="size-4" />重新同步</Button>}
              <Button type="submit" disabled={saving || editBusy}>
                {saving && <LoaderCircle className="size-4 animate-spin" />}
                {dialogUser ? "保存修改" : "创建用户"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={passwordUser !== null} onOpenChange={(open) => { if (!open && !passwordSaving) { setPasswordUser(null); setPasswordValue("") } }}>
        <DialogContent className="sm:max-w-[420px]">
          <DialogHeader>
            <DialogTitle>设置「{passwordUser?.name ?? "用户"}」的用户中心密码</DialogTitle>
            <DialogDescription>密码只保存 Argon2 hash，不会显示在用户资料中。保存后会撤销该用户已有的用户中心 session。</DialogDescription>
          </DialogHeader>
          <form onSubmit={(event) => void submitPassword(event)} className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="proxy-user-reset-password">新密码</Label>
              <Input id="proxy-user-reset-password" type="password" autoComplete="new-password" maxLength={1024} required value={passwordValue} disabled={passwordSaving} onChange={(event) => setPasswordValue(event.target.value)} />
            </div>
            <DialogFooter>
              <Button type="button" variant="outline" disabled={passwordSaving} onClick={() => setPasswordUser(null)}>取消</Button>
              <Button type="submit" disabled={passwordSaving || !passwordUser}>
                {passwordSaving && <LoaderCircle className="size-4 animate-spin" />}
                保存新密码
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <AlertDialog open={confirm !== null} onOpenChange={(open) => !open && setConfirm(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {confirm?.kind === "traffic" ? "清空用户流量？" : confirm?.kind === "delete" ? proxyUserDeleteConfirmation(confirm.user.name).title : confirm?.kind === "regenerate" ? proxyUserRegenerateConfirmation(confirm.user.name).title : "确认操作"}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {confirm?.kind === "traffic"
                ? `将清空「${confirm.user.name}」当前周期累计流量，并以各服务器下一次统计数据重新建立基线。`
                : confirm?.kind === "delete" ? proxyUserDeleteConfirmation(confirm.user.name).description
                  : confirm?.kind === "regenerate" ? proxyUserRegenerateConfirmation(confirm.user.name).description : ""}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={confirm ? busyIds.includes(confirm.user.id) : false}>取消</AlertDialogCancel>
            <AlertDialogAction
              disabled={confirm ? busyIds.includes(confirm.user.id) : false}
              onClick={(event) => { event.preventDefault(); void confirmAction() }}
              className={confirm?.kind === "delete" ? "bg-destructive text-white hover:bg-destructive/90" : confirm?.kind === "traffic" ? "bg-amber-600 text-white hover:bg-amber-700" : ""}
            >
              {confirm?.kind === "traffic" ? "确认清空" : confirm?.kind === "delete" ? "删除用户" : "重新生成 UUID"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
