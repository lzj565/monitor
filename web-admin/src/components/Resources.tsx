import { useEffect, useMemo, useState } from "react"
import { Eye, EyeOff, MoreHorizontal, Plus, RefreshCw, Search, Trash2 } from "lucide-react"
import { toast } from "sonner"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import type { Node } from "@/lib/api"
import {
  createProxy,
  createUser,
  deleteProxy,
  deleteUser,
  deleteUserProxy,
  listAllProxies,
  listUserProxies,
  listUsers,
  saveUserProxy,
  updateProxy,
  updateUser,
  type AccessDraft,
  type Flow,
  type Proxy,
  type ProxyDraft,
  type User,
  type UserDraft,
  type UserProxyAccess,
} from "@/lib/resources"

type Go = (to: string) => void

function PageHeading({ title, description, action }: {
  title: string
  description: string
  action?: React.ReactNode
}) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-3">
      <div className="space-y-1">
        <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
        <p className="text-sm text-muted-foreground">{description}</p>
      </div>
      {action}
    </div>
  )
}

function SearchInput({ value, onChange, placeholder }: {
  value: string
  onChange: (value: string) => void
  placeholder: string
}) {
  return (
    <div className="relative min-w-0 flex-1 sm:max-w-72">
      <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
      <Input className="pl-8" aria-label={placeholder} placeholder={placeholder} value={value} onChange={(e) => onChange(e.target.value)} />
    </div>
  )
}

function Field({ label, hint, children, className = "" }: {
  label: string
  hint?: string
  children: React.ReactNode
  className?: string
}) {
  return (
    <div className={`space-y-2 ${className}`}>
      <Label className="text-sm font-medium">{label}</Label>
      {children}
      {hint && <p className="text-xs leading-relaxed text-muted-foreground">{hint}</p>}
    </div>
  )
}

function FormSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-4 border-t pt-5 first:border-0 first:pt-0">
      <h3 className="text-sm font-medium">{title}</h3>
      {children}
    </section>
  )
}

function ActionMenu({ children }: { children: (close: () => void) => React.ReactNode }) {
  const [open, setOpen] = useState(false)
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="ghost" size="icon" title="更多操作" aria-label="更多操作"><MoreHorizontal /></Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-40 p-1">
        <div className="flex flex-col">{children(() => setOpen(false))}</div>
      </PopoverContent>
    </Popover>
  )
}

function MenuItem({ children, destructive = false, onClick }: {
  children: React.ReactNode
  destructive?: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      className={`flex items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-muted ${destructive ? "text-destructive" : ""}`}
      onClick={onClick}
    >
      {children}
    </button>
  )
}

function ConfirmDialog({ title, description, confirmLabel, onClose, onConfirm, busy = false }: {
  title: string
  description: string
  confirmLabel: string
  onClose: () => void
  onConfirm: () => void
  busy?: boolean
}) {
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription className="leading-relaxed">{description}</DialogDescription>
        </DialogHeader>
        <DialogFooter className="border-t pt-4">
          <Button variant="ghost" onClick={onClose}>取消</Button>
          <Button variant="destructive" onClick={onConfirm} disabled={busy}>{confirmLabel}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function useAllProxies(nodes: Node[]) {
  // Nodes stream every two seconds. Depend only on their IDs so ordinary metric
  // frames do not fan out into another request per node.
  const nodeKey = nodes.map((node) => node.id).join(",")
  const nodeIds = useMemo(() => nodeKey ? nodeKey.split(",").map(Number) : [], [nodeKey])
  const [revision, setRevision] = useState(0)
  const [result, setResult] = useState<{ nodeKey: string; revision: number; items: Proxy[] | null; error: string } | null>(null)

  useEffect(() => {
    let active = true
    listAllProxies(nodeIds).then((next) => {
      if (active) setResult({ nodeKey, revision, items: next, error: "" })
    }).catch((e: Error) => {
      if (active) setResult({ nodeKey, revision, items: null, error: e.message })
    })
    return () => { active = false }
  }, [nodeKey, nodeIds, revision])

  const current = result?.nodeKey === nodeKey && result.revision === revision
  return {
    items: current ? result.items : null,
    error: current ? result.error : "",
    loading: !current,
    reload: () => setRevision((value) => value + 1),
  }
}

function StatusFilter({ value, onChange, labels }: {
  value: string
  onChange: (value: string) => void
  labels: Array<[string, string]>
}) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger className="w-32" aria-label="按状态筛选"><SelectValue /></SelectTrigger>
      <SelectContent position="popper">
        {labels.map(([key, label]) => <SelectItem key={key} value={key}>{label}</SelectItem>)}
      </SelectContent>
    </Select>
  )
}

function ProxyForm({ proxy, nodes, onClose, onSaved }: {
  proxy: Proxy | null
  nodes: Node[]
  onClose: () => void
  onSaved: () => void
}) {
  const initialReality = proxy?.config.reality
  const [nodeId, setNodeId] = useState(proxy?.node_id ?? nodes[0]?.id ?? 0)
  const [name, setName] = useState(proxy?.name ?? "")
  const [addressType, setAddressType] = useState<ProxyDraft["address_type"]>(proxy?.address_type ?? "domain")
  const [address, setAddress] = useState(proxy?.address ?? "")
  const [port, setPort] = useState(String(proxy?.port ?? "24060"))
  const [enabled, setEnabled] = useState(proxy?.enabled ?? true)
  const [serverName, setServerName] = useState(initialReality?.server_name ?? "")
  const [serverPort, setServerPort] = useState(String(initialReality?.server_port ?? "443"))
  const [privateKey, setPrivateKey] = useState(initialReality?.private_key ?? "")
  const [publicKey, setPublicKey] = useState(initialReality?.public_key ?? "")
  const [shortId, setShortId] = useState(initialReality?.short_id ?? "")
  const [showPrivateKey, setShowPrivateKey] = useState(false)
  const [saving, setSaving] = useState(false)

  async function save(event: React.FormEvent) {
    event.preventDefault()
    if (!name.trim()) return toast.error("请填写代理名称")
    if (!proxy && !nodeId) return toast.error("请先添加节点")
    const numericPort = Number(port)
    const numericServerPort = Number(serverPort)
    if (!Number.isInteger(numericPort) || numericPort < 1 || numericPort > 65535) return toast.error("监听端口必须在 1 到 65535 之间")
    if (!Number.isInteger(numericServerPort) || numericServerPort < 1 || numericServerPort > 65535) return toast.error("Handshake Port 必须在 1 到 65535 之间")
    if (!shortId.trim()) return toast.error("请填写 Short ID")

    const draft: ProxyDraft = {
      name: name.trim(),
      protocol: "vless",
      address_type: addressType,
      address: address.trim(),
      port: numericPort,
      enabled,
      config: {
        reality: {
          enabled: true,
          server_name: serverName.trim(),
          server_port: numericServerPort,
          private_key: privateKey.trim(),
          public_key: publicKey.trim(),
          short_id: shortId.trim(),
        },
      },
    }

    setSaving(true)
    try {
      if (proxy) await updateProxy(proxy.id, draft)
      else await createProxy(nodeId, draft)
      toast.success(proxy ? "代理已保存" : "代理已创建")
      onClose()
      onSaved()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent onOpenAutoFocus={(e) => e.preventDefault()} className="sm:max-w-2xl">
        <DialogHeader><DialogTitle>{proxy ? "编辑代理" : "新增代理"}</DialogTitle></DialogHeader>
        <form className="space-y-5" onSubmit={save}>
          <FormSection title="基本信息">
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="代理名称 *">
                <Input autoFocus={!proxy} required maxLength={128} value={name} onChange={(e) => setName(e.target.value)} placeholder="香港 Reality" />
              </Field>
              <Field label="所属节点 *">
                <Select value={String(nodeId || "")} onValueChange={(value) => setNodeId(Number(value))} disabled={!!proxy || !nodes.length}>
                  <SelectTrigger className="w-full"><SelectValue placeholder="选择节点" /></SelectTrigger>
                  <SelectContent position="popper">
                    {nodes.map((node) => <SelectItem key={node.id} value={String(node.id)}>{node.name}{node.online ? " · 在线" : " · 离线"}</SelectItem>)}
                  </SelectContent>
                </Select>
                {proxy && <p className="text-xs text-muted-foreground">现有 API 不支持迁移代理到其他节点。</p>}
              </Field>
              <Field label="协议">
                <Input value="VLESS · Reality" disabled />
              </Field>
              <div className="flex items-end">
                <label className="flex w-full items-center justify-between rounded-lg border bg-muted/30 px-3 py-2.5">
                  <span className="text-sm font-medium">启用代理</span>
                  <Switch checked={enabled} onCheckedChange={setEnabled} />
                </label>
              </div>
            </div>
          </FormSection>

          <FormSection title="连接配置">
            <Field label="连接地址类型 *">
              <div className="flex flex-wrap gap-5 text-sm">
                {([["domain", "域名"], ["ipv4", "IPv4"], ["ipv6", "IPv6"]] as const).map(([value, label]) => (
                  <label key={value} className="flex cursor-pointer items-center gap-2">
                    <input type="radio" name="address-type" className="accent-primary" checked={addressType === value} onChange={() => setAddressType(value)} />
                    {label}
                  </label>
                ))}
              </div>
            </Field>
            <div className="grid gap-4 sm:grid-cols-[1fr_10rem]">
              <Field label="连接地址 *">
                <Input required value={address} onChange={(e) => setAddress(e.target.value)} placeholder={addressType === "domain" ? "hk.example.com" : addressType === "ipv4" ? "82.29.36.116" : "2001:db8::1"} />
              </Field>
              <Field label="监听端口 *">
                <Input required type="number" min="1" max="65535" step="1" value={port} onChange={(e) => setPort(e.target.value)} />
              </Field>
            </div>
          </FormSection>

          <FormSection title="Reality">
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Server Name / Handshake Server *" className="sm:col-span-2" hint="V1 使用同一个地址作为 Reality handshake server。">
                <Input required value={serverName} onChange={(e) => setServerName(e.target.value)} placeholder="www.apple.com" />
              </Field>
              <Field label="Handshake Port *">
                <Input required type="number" min="1" max="65535" step="1" value={serverPort} onChange={(e) => setServerPort(e.target.value)} />
              </Field>
              <Field label="Short ID *">
                <Input required maxLength={8} pattern="[0-9a-fA-F]{1,8}" value={shortId} onChange={(e) => setShortId(e.target.value)} placeholder="abcdef12" />
              </Field>
              <Field label="Private Key *" className="sm:col-span-2" hint="填写 sing-box Reality 密钥对中的私钥。">
                <div className="relative">
                  <Input required autoComplete="off" pattern="[A-Za-z0-9_-]{43}" type={showPrivateKey ? "text" : "password"} value={privateKey} onChange={(e) => setPrivateKey(e.target.value)} className="pr-10" />
                  <Button type="button" variant="ghost" size="icon" className="absolute top-0 right-0" title={showPrivateKey ? "隐藏私钥" : "显示私钥"} onClick={() => setShowPrivateKey((shown) => !shown)}>
                    {showPrivateKey ? <EyeOff /> : <Eye />}
                  </Button>
                </div>
              </Field>
              <Field label="Public Key *" className="sm:col-span-2" hint="与 Private Key 对应的公钥；V1 表单不提供 Reality 密钥生成。">
                <Input required autoComplete="off" pattern="[A-Za-z0-9_-]{43}" value={publicKey} onChange={(e) => setPublicKey(e.target.value)} />
              </Field>
            </div>
          </FormSection>

          <p className="text-xs leading-relaxed text-muted-foreground">保存只修改面板中的期望配置，不会自动检查或应用到服务器。</p>
          <DialogFooter className="border-t pt-4">
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving || (!proxy && !nodes.length)}>{proxy ? "保存代理" : "创建代理"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function ProxyPage({ nodes }: { nodes: Node[] }) {
  const { items, error, loading, reload } = useAllProxies(nodes)
  const [query, setQuery] = useState("")
  const [nodeFilter, setNodeFilter] = useState("all")
  const [statusFilter, setStatusFilter] = useState("all")
  const [creating, setCreating] = useState(false)
  const [editing, setEditing] = useState<Proxy | null>(null)
  const [deleting, setDeleting] = useState<Proxy | null>(null)
  const [removing, setRemoving] = useState(false)
  const nodeById = new Map(nodes.map((node) => [node.id, node]))
  const nodeOrder = new Map(nodes.map((node, index) => [node.id, index]))
  const visible = (items ?? []).filter((proxy) => {
    const node = nodeById.get(proxy.node_id)
    const needle = query.trim().toLowerCase()
    const matchesQuery = !needle || [proxy.name, proxy.address, node?.name, String(proxy.port)].some((value) => value?.toLowerCase().includes(needle))
    const matchesNode = nodeFilter === "all" || String(proxy.node_id) === nodeFilter
    const matchesStatus = statusFilter === "all" || (statusFilter === "enabled" ? proxy.enabled : !proxy.enabled)
    return matchesQuery && matchesNode && matchesStatus
  }).sort((a, b) => (nodeOrder.get(a.node_id) ?? Number.MAX_SAFE_INTEGER) - (nodeOrder.get(b.node_id) ?? Number.MAX_SAFE_INTEGER) || a.id - b.id)

  async function remove() {
    if (!deleting) return
    setRemoving(true)
    try {
      await deleteProxy(deleting.id)
      toast.success("代理及其授权已删除")
      setDeleting(null)
      reload()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setRemoving(false)
    }
  }

  return (
    <div className="space-y-4">
      <PageHeading
        title="代理"
        description="管理服务器上的 sing-box 代理入口。保存代理只更新配置数据，不会自动应用到节点。"
        action={<Button disabled={!nodes.length} onClick={() => setCreating(true)}><Plus /> 新增代理</Button>}
      />
      <div className="flex flex-wrap items-center gap-2">
        <SearchInput value={query} onChange={setQuery} placeholder="搜索代理名称、地址或节点" />
        <Select value={nodeFilter} onValueChange={setNodeFilter}>
          <SelectTrigger className="w-36" aria-label="按节点筛选"><SelectValue /></SelectTrigger>
          <SelectContent position="popper">
            <SelectItem value="all">全部节点</SelectItem>
            {nodes.map((node) => <SelectItem key={node.id} value={String(node.id)}>{node.name}</SelectItem>)}
          </SelectContent>
        </Select>
        <StatusFilter value={statusFilter} onChange={setStatusFilter} labels={[["all", "全部状态"], ["enabled", "已启用"], ["disabled", "已停用"]]} />
      </div>

      <Card className="overflow-x-auto p-0">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-[20%]">名称</TableHead>
              <TableHead className="w-[18%]">节点</TableHead>
              <TableHead className="w-[12%]">协议</TableHead>
              <TableHead className="w-[24%]">地址</TableHead>
              <TableHead className="w-[10%]">端口</TableHead>
              <TableHead className="w-[12%]">状态</TableHead>
              <TableHead className="text-right">操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {visible.map((proxy) => {
              const node = nodeById.get(proxy.node_id)
              return (
                <TableRow key={proxy.id}>
                  <TableCell>
                    <div className="font-medium">{proxy.name}</div>
                    <div className="text-xs text-muted-foreground">VLESS · Reality</div>
                  </TableCell>
                  <TableCell>
                    <div className="font-medium">{node?.name ?? `节点 ${proxy.node_id}`}</div>
                    <Badge variant={node?.online ? "default" : "secondary"} className="mt-1 font-normal">{node?.online ? "在线" : "离线"}</Badge>
                  </TableCell>
                  <TableCell><Badge variant="outline" className="font-normal">VLESS</Badge></TableCell>
                  <TableCell>
                    <div className="max-w-56 truncate text-sm" title={proxy.address}>{proxy.address}</div>
                    <div className="text-xs text-muted-foreground">{proxy.address_type === "domain" ? "Domain" : proxy.address_type.toUpperCase()}</div>
                  </TableCell>
                  <TableCell className="tnum">{proxy.port}</TableCell>
                  <TableCell><Badge variant={proxy.enabled ? "default" : "secondary"} className="font-normal">{proxy.enabled ? "● 启用" : "○ 停用"}</Badge></TableCell>
                  <TableCell className="text-right">
                    <ActionMenu>
                      {(close) => <>
                        <MenuItem onClick={() => { close(); setEditing(proxy) }}>编辑</MenuItem>
                        <MenuItem destructive onClick={() => { close(); setDeleting(proxy) }}><Trash2 className="size-4" /> 删除</MenuItem>
                      </>}
                    </ActionMenu>
                  </TableCell>
                </TableRow>
              )
            })}
            {!loading && !error && !visible.length && (
              <TableRow><TableCell colSpan={7} className="py-10 text-center text-sm text-muted-foreground">
                {!items?.length ? (nodes.length ? "还没有代理，右上角新增" : "先添加节点，再创建代理") : "没有匹配的代理"}
              </TableCell></TableRow>
            )}
            {loading && <TableRow><TableCell colSpan={7} className="py-10 text-center text-sm text-muted-foreground">正在加载代理…</TableCell></TableRow>}
            {error && <TableRow><TableCell colSpan={7} className="py-8 text-center text-sm text-destructive">
              <div role="alert">加载代理失败：{error}</div>
              <Button variant="outline" size="sm" className="mt-3" onClick={reload}>重试</Button>
            </TableCell></TableRow>}
          </TableBody>
        </Table>
      </Card>

      {creating && <ProxyForm nodes={nodes} proxy={null} onClose={() => setCreating(false)} onSaved={reload} />}
      {editing && <ProxyForm nodes={nodes} proxy={editing} onClose={() => setEditing(null)} onSaved={reload} />}
      {deleting && <ConfirmDialog
        title={`删除代理「${deleting.name}」？`}
        description="删除会一并解除所有关联用户授权。此操作只修改配置数据，不会自动应用到服务器。"
        confirmLabel="删除代理"
        busy={removing}
        onClose={() => setDeleting(null)}
        onConfirm={remove}
      />}
    </div>
  )
}

function localDate(timestamp: number | null): string {
  if (timestamp === null) return ""
  // expires_at is an exclusive boundary; subtract one second to show the last
  // calendar date on which the account remains usable.
  const date = new Date(timestamp * 1000 - 1000)
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, "0")
  const day = String(date.getDate()).padStart(2, "0")
  return `${year}-${month}-${day}`
}

function localExpiryBoundary(date: string): number | null {
  if (!date) return null
  const [year, month, day] = date.split("-").map(Number)
  return Math.floor(new Date(year, month - 1, day + 1, 0, 0, 0).getTime() / 1000)
}

function displayDate(timestamp: number | null): string {
  if (timestamp === null) return "永不过期"
  const date = new Date(timestamp * 1000 - 1000)
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`
}

function expired(user: User): boolean {
  return user.expires_at !== null && user.expires_at * 1000 <= Date.now()
}

function UserStatus({ user }: { user: User }) {
  return (
    <div className="flex flex-wrap gap-1">
      <Badge variant={user.enabled ? "default" : "secondary"} className="font-normal">{user.enabled ? "已启用" : "已停用"}</Badge>
      {expired(user) && <Badge variant="destructive" className="font-normal">已过期</Badge>}
    </div>
  )
}

function UserForm({ user, onClose, onSaved }: {
  user: User | null
  onClose: () => void
  onSaved: () => void
}) {
  const [username, setUsername] = useState(user?.username ?? "")
  const [enabled, setEnabled] = useState(user?.enabled ?? true)
  const [forever, setForever] = useState(user?.expires_at === null || !user)
  const [expires, setExpires] = useState(localDate(user?.expires_at ?? null))
  const [saving, setSaving] = useState(false)

  async function save(event: React.FormEvent) {
    event.preventDefault()
    if (!username.trim()) return toast.error("请填写用户名")
    if (!forever && !expires) return toast.error("请选择到期日期")
    const draft: UserDraft = { username: username.trim(), enabled, expires_at: forever ? null : localExpiryBoundary(expires) }
    setSaving(true)
    try {
      if (user) await updateUser(user.id, draft)
      else await createUser(draft)
      toast.success(user ? "用户已保存" : "用户已创建")
      onClose()
      onSaved()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent onOpenAutoFocus={(e) => e.preventDefault()} className="sm:max-w-xl">
        <DialogHeader><DialogTitle>{user ? "编辑用户" : "新增用户"}</DialogTitle></DialogHeader>
        <form className="space-y-5" onSubmit={save}>
          <FormSection title="用户信息">
            <Field label="用户名 *">
              <Input autoFocus={!user} required maxLength={128} value={username} onChange={(e) => setUsername(e.target.value)} />
            </Field>
            <label className="flex items-center justify-between rounded-lg border bg-muted/30 px-3 py-2.5">
              <span className="text-sm font-medium">启用用户</span>
              <Switch checked={enabled} onCheckedChange={setEnabled} />
            </label>
            <Field label="过期时间">
              <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
                <label className="flex items-center gap-2 text-sm">
                  <input type="radio" name="expiry" className="accent-primary" checked={forever} onChange={() => setForever(true)} />
                  永不过期
                </label>
                <label className="flex items-center gap-2 text-sm">
                  <input type="radio" name="expiry" className="accent-primary" checked={!forever} onChange={() => setForever(false)} />
                  指定日期
                </label>
                <Input className="sm:max-w-52" type="date" disabled={forever} value={expires} onChange={(e) => setExpires(e.target.value)} />
              </div>
              {!forever && <p className="text-xs text-muted-foreground">所选日期当天结束时到期，按当前浏览器时区计算。</p>}
            </Field>
          </FormSection>
          {!user && <p className="text-sm text-muted-foreground">创建用户后，可在“订阅”页面配置代理访问权限。</p>}
          <DialogFooter className="border-t pt-4">
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving}>{user ? "保存用户" : "创建用户"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function UsersPage({ go }: { go: Go }) {
  const [revision, setRevision] = useState(0)
  const [result, setResult] = useState<{ revision: number; items: User[] | null; error: string } | null>(null)
  const [query, setQuery] = useState("")
  const [status, setStatus] = useState("all")
  const [creating, setCreating] = useState(false)
  const [editing, setEditing] = useState<User | null>(null)
  const [deleting, setDeleting] = useState<User | null>(null)
  const [removing, setRemoving] = useState(false)

  useEffect(() => {
    let active = true
    listUsers().then((users) => {
      if (active) setResult({ revision, items: users, error: "" })
    }).catch((e: Error) => {
      if (active) setResult({ revision, items: null, error: e.message })
    })
    return () => { active = false }
  }, [revision])

  const current = result?.revision === revision
  const items = current ? result.items : null
  const error = current ? result.error : ""
  const loading = !current
  const needle = query.trim().toLowerCase()
  const visible = (items ?? []).filter((user) => {
    const matchesText = !needle || user.username.toLowerCase().includes(needle)
    const matchesStatus = status === "all"
      || (status === "enabled" && user.enabled)
      || (status === "disabled" && !user.enabled)
      || (status === "expired" && expired(user))
    return matchesText && matchesStatus
  })

  async function remove() {
    if (!deleting) return
    setRemoving(true)
    try {
      await deleteUser(deleting.id)
      toast.success("用户及其授权已删除")
      setDeleting(null)
      setRevision((value) => value + 1)
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setRemoving(false)
    }
  }

  return (
    <div className="space-y-4">
      <PageHeading title="用户" description="管理代理用户及使用权限。" action={<Button onClick={() => setCreating(true)}><Plus /> 新增用户</Button>} />
      <div className="flex flex-wrap items-center gap-2">
        <SearchInput value={query} onChange={setQuery} placeholder="搜索用户名" />
        <StatusFilter value={status} onChange={setStatus} labels={[["all", "全部状态"], ["enabled", "已启用"], ["disabled", "已停用"], ["expired", "已过期"]]} />
      </div>
      <Card className="overflow-x-auto p-0">
        <Table>
          <TableHeader><TableRow>
            <TableHead className="w-[30%]">用户</TableHead>
            <TableHead className="w-[20%]">代理授权</TableHead>
            <TableHead className="w-[22%]">到期时间</TableHead>
            <TableHead className="w-[20%]">状态</TableHead>
            <TableHead className="text-right">操作</TableHead>
          </TableRow></TableHeader>
          <TableBody>
            {visible.map((user) => (
              <TableRow key={user.id}>
                <TableCell><div className="font-medium">{user.username}</div></TableCell>
                <TableCell>
                  <button type="button" className="text-sm text-primary underline-offset-4 hover:underline" onClick={() => go(`/admin/subscriptions?user_id=${user.id}`)}>
                    {user.proxy_count} 个代理
                  </button>
                </TableCell>
                <TableCell className="tnum text-sm">{displayDate(user.expires_at)}</TableCell>
                <TableCell><UserStatus user={user} /></TableCell>
                <TableCell className="text-right">
                  <ActionMenu>{(close) => <>
                    <MenuItem onClick={() => { close(); setEditing(user) }}>编辑</MenuItem>
                    <MenuItem destructive onClick={() => { close(); setDeleting(user) }}><Trash2 className="size-4" /> 删除</MenuItem>
                  </>}</ActionMenu>
                </TableCell>
              </TableRow>
            ))}
            {!loading && !error && !visible.length && <TableRow><TableCell colSpan={5} className="py-10 text-center text-sm text-muted-foreground">{items?.length ? "没有匹配的用户" : "还没有用户，右上角新增"}</TableCell></TableRow>}
            {loading && <TableRow><TableCell colSpan={5} className="py-10 text-center text-sm text-muted-foreground">正在加载用户…</TableCell></TableRow>}
            {error && <TableRow><TableCell colSpan={5} className="py-8 text-center text-sm text-destructive">
              <div role="alert">加载用户失败：{error}</div>
              <Button variant="outline" size="sm" className="mt-3" onClick={() => setRevision((value) => value + 1)}>重试</Button>
            </TableCell></TableRow>}
          </TableBody>
        </Table>
      </Card>
      {creating && <UserForm user={null} onClose={() => setCreating(false)} onSaved={() => setRevision((value) => value + 1)} />}
      {editing && <UserForm user={editing} onClose={() => setEditing(null)} onSaved={() => setRevision((value) => value + 1)} />}
      {deleting && <ConfirmDialog
        title={`删除用户「${deleting.username}」？`}
        description={`此用户的 ${deleting.proxy_count} 条代理授权也会删除。此操作只修改配置数据，不会自动应用到服务器。`}
        confirmLabel="删除用户"
        busy={removing}
        onClose={() => setDeleting(null)}
        onConfirm={remove}
      />}
    </div>
  )
}

function AccessForm({ user, proxies, nodes, access, onClose, onSaved }: {
  user: User
  proxies: Proxy[]
  nodes: Node[]
  access: UserProxyAccess | null
  onClose: () => void
  onSaved: () => void
}) {
  const [proxyId, setProxyId] = useState(access ? String(access.proxy.id) : "")
  const [enabled, setEnabled] = useState(access?.access.enabled ?? true)
  const [uuid, setUuid] = useState(access?.access.auth.uuid ?? crypto.randomUUID())
  const [flow, setFlow] = useState<Flow>(access?.access.auth.flow ?? "")
  const [saving, setSaving] = useState(false)
  const selectedProxy = proxies.find((proxy) => proxy.id === Number(proxyId))
  const node = selectedProxy ? nodes.find((item) => item.id === selectedProxy.node_id) : undefined

  async function save(event: React.FormEvent) {
    event.preventDefault()
    if (!access && !proxyId) return toast.error("请选择代理")
    if (!uuid.trim()) return toast.error("请填写 UUID")
    const draft: AccessDraft = { enabled, auth: { uuid: uuid.trim(), flow } }
    setSaving(true)
    try {
      await saveUserProxy(user.id, Number(proxyId), draft)
      toast.success(access ? "代理授权已保存" : "代理授权已添加")
      onClose()
      onSaved()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent onOpenAutoFocus={(e) => e.preventDefault()} className="sm:max-w-xl">
        <DialogHeader><DialogTitle>{access ? "编辑代理授权" : "添加代理授权"}</DialogTitle></DialogHeader>
        <form className="space-y-5" onSubmit={save}>
          <FormSection title="用户">
            <div className="rounded-lg border bg-muted/30 px-3 py-2.5 text-sm font-medium">{user.username}</div>
          </FormSection>
          <FormSection title="代理">
            {access ? (
              <div className="rounded-lg border bg-muted/30 px-3 py-2.5">
                <div className="text-sm font-medium">{access.proxy.name}</div>
                <div className="mt-0.5 text-xs text-muted-foreground">{nodes.find((item) => item.id === access.proxy.node_id)?.name ?? `节点 ${access.proxy.node_id}`} · {access.proxy.protocol.toUpperCase()} · {access.proxy.address}:{access.proxy.port}</div>
              </div>
            ) : (
              <Field label="代理 *">
                <Select value={proxyId} onValueChange={setProxyId}>
                  <SelectTrigger className="w-full"><SelectValue placeholder="选择代理" /></SelectTrigger>
                  <SelectContent position="popper">
                    {proxies.map((proxy) => <SelectItem key={proxy.id} value={String(proxy.id)}>{proxy.name} · {nodes.find((item) => item.id === proxy.node_id)?.name ?? `节点 ${proxy.node_id}`} · {proxy.address}:{proxy.port}</SelectItem>)}
                  </SelectContent>
                </Select>
                {!proxies.length && <p className="text-xs text-muted-foreground">该用户已授权所有代理，或目前没有可用代理。</p>}
              </Field>
            )}
            {selectedProxy && !access && <p className="text-xs text-muted-foreground">节点：{node?.name ?? `节点 ${selectedProxy.node_id}`}</p>}
          </FormSection>
          <FormSection title="认证信息">
            <label className="flex items-center justify-between rounded-lg border bg-muted/30 px-3 py-2.5">
              <span className="text-sm font-medium">启用授权</span>
              <Switch checked={enabled} onCheckedChange={setEnabled} />
            </label>
            <Field label="UUID *">
              <div className="flex gap-2">
                <Input required pattern="[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}" value={uuid} onChange={(e) => setUuid(e.target.value)} />
                <Button type="button" variant="outline" size="icon" title="生成 UUID" aria-label="生成 UUID" onClick={() => setUuid(crypto.randomUUID())}><RefreshCw /></Button>
              </div>
            </Field>
            <Field label="Flow">
              <Select value={flow || "none"} onValueChange={(value) => setFlow(value === "none" ? "" : value as Flow)}>
                <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                <SelectContent position="popper">
                  <SelectItem value="none">无</SelectItem>
                  <SelectItem value="xtls-rprx-vision">xtls-rprx-vision</SelectItem>
                </SelectContent>
              </Select>
            </Field>
          </FormSection>
          <DialogFooter className="border-t pt-4">
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving || (!access && !proxies.length)}>{access ? "保存" : "添加授权"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function SubscriptionsPage({ nodes, search, go }: { nodes: Node[]; search: string; go: Go }) {
  const { items: proxies, error: proxyError, loading: proxiesLoading, reload: reloadProxies } = useAllProxies(nodes)
  const [usersRevision, setUsersRevision] = useState(0)
  const [usersResult, setUsersResult] = useState<{ revision: number; items: User[] | null; error: string } | null>(null)
  const [userQuery, setUserQuery] = useState("")
  const [proxyQuery, setProxyQuery] = useState("")
  const [nodeFilter, setNodeFilter] = useState("all")
  const [accessRevision, setAccessRevision] = useState(0)
  const [accessResult, setAccessResult] = useState<{ userId: number; revision: number; items: UserProxyAccess[] | null; error: string } | null>(null)
  const [adding, setAdding] = useState(false)
  const [editing, setEditing] = useState<UserProxyAccess | null>(null)
  const [deleting, setDeleting] = useState<UserProxyAccess | null>(null)
  const [removing, setRemoving] = useState(false)
  const currentUsers = usersResult?.revision === usersRevision
  const users = usersResult?.items ?? null
  const usersLoading = !currentUsers
  const usersError = currentUsers ? usersResult?.error ?? "" : ""
  const requestedId = new URLSearchParams(search).get("user_id")
  const requestedUserId = requestedId && /^\d+$/.test(requestedId) ? Number(requestedId) : null
  const selectedId = requestedUserId !== null && users?.some((user) => user.id === requestedUserId)
    ? requestedUserId
    : users?.[0]?.id ?? null
  const selectedUser = users?.find((user) => user.id === selectedId) ?? null
  const currentAccess = selectedId !== null && accessResult?.userId === selectedId && accessResult.revision === accessRevision
  const accesses = selectedId === null ? [] : currentAccess ? accessResult?.items ?? null : null
  const accessLoading = selectedId !== null && !currentAccess
  const accessError = currentAccess ? accessResult?.error ?? "" : ""
  const nodeById = new Map(nodes.map((node) => [node.id, node]))
  const availableProxies = (proxies ?? []).filter((proxy) => !accesses?.some((item) => item.proxy.id === proxy.id))
  const userNeedle = userQuery.trim().toLowerCase()
  const visibleUsers = (users ?? []).filter((user) => user.username.toLowerCase().includes(userNeedle))
  const proxyNeedle = proxyQuery.trim().toLowerCase()
  const visibleAccesses = (accesses ?? []).filter((item) => {
    const node = nodeById.get(item.proxy.node_id)
    const matchesText = !proxyNeedle || [item.proxy.name, item.proxy.address, node?.name].some((value) => value?.toLowerCase().includes(proxyNeedle))
    return matchesText && (nodeFilter === "all" || String(item.proxy.node_id) === nodeFilter)
  })

  useEffect(() => {
    let active = true
    listUsers().then((next) => {
      if (active) setUsersResult({ revision: usersRevision, items: next, error: "" })
    }).catch((e: Error) => {
      if (active) setUsersResult({ revision: usersRevision, items: null, error: e.message })
    })
    return () => { active = false }
  }, [usersRevision])

  useEffect(() => {
    if (selectedId === null) return
    let active = true
    listUserProxies(selectedId).then((next) => {
      if (active) setAccessResult({ userId: selectedId, revision: accessRevision, items: next, error: "" })
    }).catch((e: Error) => {
      if (active) setAccessResult({ userId: selectedId, revision: accessRevision, items: null, error: e.message })
    })
    return () => { active = false }
  }, [selectedId, accessRevision])

  function selectUser(userId: number) {
    go(`/admin/subscriptions?user_id=${userId}`)
  }

  function reloadAccesses() {
    setAccessRevision((value) => value + 1)
    setUsersRevision((value) => value + 1)
    reloadProxies()
  }

  async function toggleAccess(item: UserProxyAccess) {
    if (!selectedUser) return
    try {
      await saveUserProxy(selectedUser.id, item.proxy.id, {
        enabled: !item.access.enabled,
        auth: item.access.auth,
      })
      toast.success(item.access.enabled ? "授权已停用" : "授权已启用")
      reloadAccesses()
    } catch (e) {
      toast.error((e as Error).message)
    }
  }

  async function removeAccess() {
    if (!selectedUser || !deleting) return
    setRemoving(true)
    try {
      await deleteUserProxy(selectedUser.id, deleting.proxy.id)
      toast.success("代理授权已删除")
      setDeleting(null)
      reloadAccesses()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setRemoving(false)
    }
  }

  return (
    <div className="space-y-4">
      <PageHeading title="订阅" description="管理用户可访问的代理节点和连接凭证。授权或删除只修改配置数据，不会自动应用到服务器。" />
      <div className="grid min-h-[32rem] gap-4 lg:grid-cols-[18rem_minmax(0,1fr)]">
        <Card className="gap-0 overflow-hidden p-0">
          <div className="border-b p-4">
            <h2 className="mb-3 text-sm font-medium">用户</h2>
            <SearchInput value={userQuery} onChange={setUserQuery} placeholder="搜索用户" />
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto p-2">
            {usersLoading && <p className="p-3 text-sm text-muted-foreground">正在加载用户…</p>}
            {usersError && <div className="p-3 text-sm text-destructive" role="alert">{usersError}<Button variant="link" className="h-auto p-0 pl-2" onClick={() => setUsersRevision((value) => value + 1)}>重试</Button></div>}
            {!usersLoading && !usersError && !visibleUsers.length && <p className="p-3 text-sm text-muted-foreground">{users?.length ? "没有匹配的用户" : "还没有用户"}</p>}
            {visibleUsers.map((user) => (
              <button
                key={user.id}
                type="button"
                aria-current={selectedId === user.id ? "true" : undefined}
                className={`flex w-full items-center justify-between gap-3 rounded-md px-3 py-2 text-left text-sm transition-colors ${selectedId === user.id ? "bg-secondary font-medium" : "hover:bg-muted"}`}
                onClick={() => selectUser(user.id)}
              >
                <span className="min-w-0 truncate">{user.username}</span>
                <span className="tnum text-xs text-muted-foreground">{user.proxy_count}</span>
              </button>
            ))}
          </div>
        </Card>

        <div className="min-w-0 space-y-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h2 className="text-lg font-semibold">{selectedUser?.username ?? "选择用户"}</h2>
              {selectedUser && <div className="mt-1 flex flex-wrap items-center gap-2"><p className="text-sm text-muted-foreground">已授权 {selectedUser.proxy_count} 个代理</p><UserStatus user={selectedUser} /></div>}
            </div>
            <Button disabled={!selectedUser || usersLoading || proxiesLoading || accessLoading || accesses === null || proxies === null || !availableProxies.length || !!proxyError || !!accessError} onClick={() => setAdding(true)}><Plus /> 添加授权</Button>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <SearchInput value={proxyQuery} onChange={setProxyQuery} placeholder="搜索代理" />
            <Select value={nodeFilter} onValueChange={setNodeFilter}>
              <SelectTrigger className="w-36" aria-label="按节点筛选"><SelectValue /></SelectTrigger>
              <SelectContent position="popper">
                <SelectItem value="all">全部节点</SelectItem>
                {nodes.map((node) => <SelectItem key={node.id} value={String(node.id)}>{node.name}</SelectItem>)}
              </SelectContent>
            </Select>
          </div>

          {proxyError && <div className="rounded-lg border p-4 text-sm text-destructive" role="alert">加载代理失败：{proxyError}<Button variant="link" className="h-auto p-0 pl-2" onClick={reloadProxies}>重试</Button></div>}
          {accessError && <div className="rounded-lg border p-4 text-sm text-destructive" role="alert">加载授权失败：{accessError}<Button variant="link" className="h-auto p-0 pl-2" onClick={() => setAccessRevision((value) => value + 1)}>重试</Button></div>}
          {(proxiesLoading || accessLoading) && <Card className="p-6 text-sm text-muted-foreground">正在加载授权…</Card>}
          {!proxiesLoading && !accessLoading && !proxyError && !accessError && selectedUser && (
            <Card className="gap-0 overflow-hidden p-0">
              {visibleAccesses.map((item) => {
                const node = nodeById.get(item.proxy.node_id)
                return (
                  <div key={item.proxy.id} className="flex flex-col gap-3 border-b p-4 last:border-0 sm:flex-row sm:items-start sm:justify-between">
                    <div className="min-w-0 space-y-1.5">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-medium">{item.proxy.name}</span>
                        <Badge variant={item.access.enabled ? "default" : "secondary"} className="font-normal">授权{item.access.enabled ? "启用" : "停用"}</Badge>
                        <Badge variant={item.proxy.enabled ? "outline" : "secondary"} className="font-normal">代理{item.proxy.enabled ? "启用" : "停用"}</Badge>
                      </div>
                      <div className="text-sm text-muted-foreground">{node?.name ?? `节点 ${item.proxy.node_id}`} · {item.proxy.protocol.toUpperCase()} · {item.proxy.address}:{item.proxy.port}</div>
                      <div className="break-all font-mono text-xs text-muted-foreground">UUID {item.access.auth.uuid}</div>
                      <div className="text-xs text-muted-foreground">Flow {item.access.auth.flow || "无"}</div>
                    </div>
                    <div className="self-end sm:self-start">
                      <ActionMenu>{(close) => <>
                        <MenuItem onClick={() => { close(); setEditing(item) }}>编辑</MenuItem>
                        <MenuItem onClick={() => { close(); void toggleAccess(item) }}>{item.access.enabled ? "停用" : "启用"}</MenuItem>
                        <div className="my-1 border-t" />
                        <MenuItem destructive onClick={() => { close(); setDeleting(item) }}><Trash2 className="size-4" /> 删除授权</MenuItem>
                      </>}</ActionMenu>
                    </div>
                  </div>
                )
              })}
              {!visibleAccesses.length && <div className="p-10 text-center text-sm text-muted-foreground">{accesses?.length ? "没有匹配的授权" : "该用户还没有代理授权"}</div>}
            </Card>
          )}
        </div>
      </div>

      {adding && selectedUser && <AccessForm user={selectedUser} proxies={availableProxies} nodes={nodes} access={null} onClose={() => setAdding(false)} onSaved={reloadAccesses} />}
      {editing && selectedUser && <AccessForm user={selectedUser} proxies={proxies ?? []} nodes={nodes} access={editing} onClose={() => setEditing(null)} onSaved={reloadAccesses} />}
      {deleting && selectedUser && <ConfirmDialog
        title="删除代理授权？"
        description={`删除后 ${selectedUser.username} 将不再关联「${deleting.proxy.name}」。此操作只修改配置数据，不会自动应用到服务器。`}
        confirmLabel="删除授权"
        busy={removing}
        onClose={() => setDeleting(null)}
        onConfirm={removeAccess}
      />}
    </div>
  )
}

export function ProxiesPage({ nodes }: { nodes: Node[] }) {
  return <ProxyPage nodes={nodes} />
}

export function UsersResourcePage({ go }: { go: Go }) {
  return <UsersPage go={go} />
}

export function SubscriptionsResourcePage({ nodes, search, go }: { nodes: Node[]; search: string; go: Go }) {
  return <SubscriptionsPage nodes={nodes} search={search} go={go} />
}
