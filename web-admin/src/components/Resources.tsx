import { useAdminResources, useSharedProxies, useSharedUsers } from "@/components/AdminResources"
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { AlertTriangle, ChevronDown, ChevronRight, ChevronUp, Copy, Eye, EyeOff, Pencil, Plus, RefreshCw, Settings2, Trash2 } from "lucide-react"
import { toast } from "sonner"

import { TrafficUsage } from "@/components/TrafficUsage"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { Skeleton } from "@/components/ui/skeleton"
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip"
import { AdminConfirmDialog as ConfirmDialog, AdminSearchInput } from "@/components/AdminShared"
import { DragHandle, useDragOrder } from "@/components/DragOrder"
import type { Node } from "@/lib/api"
import { bytes } from "@/lib/format"
import { parseResetDay, parseUserLimits } from "@/lib/user-limits"
import { proxyDraft } from "@/lib/proxy-draft"
import { countryFlag, displayProxyName } from "@/lib/proxy-name"
import { ProxyAddressTypeBadge, ProxyProtocolBadge, TooltipText } from "@/components/ProxyBadges"
import { generateRealityKeyPair, generateShortId, realityKeyPairMatches } from "@/lib/reality"
import { formatSni, parseDestination, parseSni } from "@/lib/sni"
import { groupProxiesByNode, proxyGroupSelection, subscriptionSearchMatches, toggleProxyGroup } from "@/lib/subscription"
import {
  createProxy,
  createUser,
  deleteProxy,
  deleteUser,
  getProxyUserTraffic,
  listProxyUserTraffic,
  listUserAuthorizations,
  replaceUserAuthorizations,
  resolveSync,
  resetUserUuid,
  resetProxyUserTraffic,
  updateProxy,
  updateUser,
  type Flow,
  type Proxy,
  type ProxyDraft,
  type SyncReport,
  type User,
  type UserDraft,
  type UserProxyAuthorization,
  type ProxyTrafficSummary,
} from "@/lib/resources"

function notifySync(message: string, sync?: SyncReport) {
  if (!sync) {
    toast.success(message)
    return
  }
  const queued = sync.queued.length
  const offline = sync.needs_sync.filter((item) => item.reason === "offline").length
  const otherFailures = sync.needs_sync.length - offline
  if (queued) toast.success(`${message}；已向 ${queued} 个节点下发配置，正在等待 Agent 应用`)
  else if (!sync.needs_sync.length) toast.success(message)
  if (offline) toast.warning(`${offline} 个节点当前离线，将在 Agent 重连时自动同步`)
  if (otherFailures) toast.error(`${otherFailures} 个节点未能排队同步，请检查节点配置或命令队列`)

  if (queued) {
    void resolveSync(sync).then((result) => {
      if (result.failed.length) {
        toast.error(`有 ${result.failed.length} 个节点应用配置失败：${result.failed.map((item) => `${item.node_id}（${item.message}）`).join("、")}`)
      }
      if (result.pending.length) {
        toast.warning(`${result.pending.length} 个节点仍未返回应用结果；可在节点重连后自动同步`)
      }
      if (result.succeeded.length) {
        toast.success(`配置已在 ${result.succeeded.length} 个节点应用`)
      }
    }).catch((error) => toast.error(`查询配置同步结果失败：${(error as Error).message}`))
  }
}

type Go = (to: string) => void

function copyText(value: string) {
  if (!navigator.clipboard) return toast.error("无法访问剪贴板")
  navigator.clipboard.writeText(value).then(
    () => toast.success("已复制"),
    () => toast.error("复制失败"),
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

function useAllProxies() {
  const result = useSharedProxies()
  const { proxies, users, invalidateAuthorizations } = useAdminResources()
  const reload = useCallback(() => {
    invalidateAuthorizations()
    void proxies.refresh(true)
    void users.refresh(true)
  }, [proxies, users, invalidateAuthorizations])
  return { ...result, reload }
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

function realityReady(reality: Proxy["config"]["reality"]): boolean {
  return Boolean(
    reality.server_name.trim()
    && Number.isInteger(reality.server_port) && reality.server_port > 0 && reality.server_port <= 65535
    && /^[0-9a-fA-F]{1,8}$/.test(reality.short_id)
    && /^[A-Za-z0-9_-]{43}$/.test(reality.private_key)
    && /^[A-Za-z0-9_-]{43}$/.test(reality.public_key)
    && realityKeyPairMatches(reality.private_key, reality.public_key),
  )
}

function nodeAddress(node: Node | undefined, type: "ipv4" | "ipv6"): string {
  return node?.addresses?.find(({ address }) => (type === "ipv6") === address.includes(":"))?.address ?? ""
}

function defaultAddressType(node: Node | undefined): ProxyDraft["address_type"] {
  if (nodeAddress(node, "ipv4")) return "ipv4"
  if (nodeAddress(node, "ipv6")) return "ipv6"
  return "domain"
}

function ProxyForm({ proxy, nodes, onClose, onSaved }: {
  proxy: Proxy | null
  nodes: Node[]
  onClose: () => void
  onSaved: () => void
}) {
  const initialReality = proxy?.config.reality
  const [creationPair] = useState(() => proxy ? null : generateRealityKeyPair())
  const initialNodeId = proxy?.node_id ?? nodes[0]?.id ?? 0
  const initialNode = nodes.find((node) => node.id === initialNodeId)
  const [nodeId, setNodeId] = useState(initialNodeId)
  const [name, setName] = useState(proxy?.name ?? "")
  const [includeNodeName, setIncludeNodeName] = useState(proxy?.include_node_name ?? true)
  const [addressType, setAddressType] = useState<ProxyDraft["address_type"]>(proxy?.address_type ?? defaultAddressType(initialNode))
  const [customAddress, setCustomAddress] = useState(proxy?.address_type === "domain" ? proxy.address : "")
  const [port, setPort] = useState(String(proxy?.port ?? "24060"))
  const [flow, setFlow] = useState<Flow>(proxy?.flow || "xtls-rprx-vision")
  const [sni, setSni] = useState(initialReality?.server_name ?? "www.amd.com")
  const [destination, setDestination] = useState(initialReality ? formatSni(initialReality.server || initialReality.server_name, initialReality.server_port) : "www.amd.com:443")
  const [privateKey, setPrivateKey] = useState(initialReality?.private_key ?? creationPair?.privateKey ?? "")
  const [publicKey, setPublicKey] = useState(initialReality?.public_key ?? creationPair?.publicKey ?? "")
  const [shortId, setShortId] = useState(() => initialReality?.short_id ?? (proxy ? "" : generateShortId()))
  const [showPrivateKey, setShowPrivateKey] = useState(false)
  const [advanced, setAdvanced] = useState(() => Boolean(proxy && !realityReady(proxy.config.reality)))
  const [confirmKeyPair, setConfirmKeyPair] = useState(false)
  const [saving, setSaving] = useState(false)
  const selectedNode = nodes.find((node) => node.id === nodeId)
  const address = addressType === "domain"
    ? customAddress
    : nodeAddress(selectedNode, addressType) || (proxy?.address_type === addressType ? proxy.address : "")
  const displayName = displayProxyName({ name, include_node_name: includeNodeName, node_id: nodeId }, selectedNode)
  const currentReality = {
    enabled: true,
    server_name: initialReality?.server_name ?? "",
    server: initialReality?.server ?? initialReality?.server_name ?? "",
    server_port: initialReality?.server_port ?? 443,
    private_key: privateKey.trim(),
    public_key: publicKey.trim(),
    short_id: shortId.trim(),
  }
  let parsedSni: ReturnType<typeof parseSni> | null = null
  try {
    parsedSni = parseSni(sni)
  } catch {
    // Validation is reported on submit so the user can finish editing the field.
  }
  let parsedDestination: ReturnType<typeof parseDestination> | null = null
  try { parsedDestination = parseDestination(destination) } catch { /* Report on submit. */ }
  const completeReality = Boolean(parsedSni && parsedDestination && realityKeyPairMatches(privateKey.trim(), publicKey.trim())
    && /^[0-9a-fA-F]{1,8}$/.test(shortId.trim()))

  function changeNode(value: string) {
    const nextNodeId = Number(value)
    const nextNode = nodes.find((node) => node.id === nextNodeId)
    setNodeId(nextNodeId)
    if (addressType !== "domain" && !nodeAddress(nextNode, addressType)) {
      setAddressType(defaultAddressType(nextNode))
    }
  }

  function changeAddressType(type: ProxyDraft["address_type"]) {
    setAddressType(type)
  }

  function regeneratePair() {
    const pair = generateRealityKeyPair()
    setPrivateKey(pair.privateKey)
    setPublicKey(pair.publicKey)
    setConfirmKeyPair(false)
  }

  function requestPairRegeneration() {
    if (proxy) setConfirmKeyPair(true)
    else regeneratePair()
  }

  async function save(event: React.FormEvent) {
    event.preventDefault()
    if (!name.trim()) return toast.error("请填写代理名称")
    if (!nodeId) return toast.error("请先添加节点")
    const numericPort = Number(port)
    if (!Number.isInteger(numericPort) || numericPort < 1 || numericPort > 65535) return toast.error("监听端口必须在 1 到 65535 之间")
    let parsedSniValue: ReturnType<typeof parseSni>
    let parsedDestinationValue: ReturnType<typeof parseDestination>
    try {
      parsedSniValue = parseSni(sni)
    } catch (error) {
      setAdvanced(true)
      return toast.error((error as Error).message)
    }
    try { parsedDestinationValue = parseDestination(destination) } catch (error) {
      setAdvanced(true)
      return toast.error((error as Error).message.replace("SNI", "DEST"))
    }
    if (!/^[0-9a-fA-F]{1,8}$/.test(shortId.trim())) return toast.error("Short ID 必须为 1 到 8 位十六进制字符")
    if (!realityKeyPairMatches(privateKey.trim(), publicKey.trim())) {
      setAdvanced(true)
      return toast.error("Private Key 与 Public Key 不匹配，请修正或重新生成密钥对")
    }
    if (!address.trim()) return toast.error("请填写连接地址")

    const draft: ProxyDraft = {
      name: name.trim(),
      include_node_name: includeNodeName,
      protocol: "vless",
      address_type: addressType,
      address: address.trim(),
      port: numericPort,
      enabled: proxy?.enabled ?? true,
      flow,
      config: {
        reality: { ...currentReality, ...parsedSniValue, ...parsedDestinationValue },
      },
    }

    setSaving(true)
    try {
      const saved = proxy
        ? await updateProxy(proxy.id, draft, nodeId)
        : await createProxy(nodeId, draft)
      notifySync(proxy ? "代理配置已更新" : "代理已创建", saved.sync)
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
      <DialogContent onOpenAutoFocus={(event) => event.preventDefault()} className="flex max-h-[90vh] w-full max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-md">
        <form className="flex min-h-0 flex-1 flex-col" onSubmit={save}>
          <DialogHeader className="shrink-0 px-6 pt-5 pb-3">
            <DialogTitle className="pr-6 [overflow-wrap:anywhere]">{proxy ? `编辑代理：${displayProxyName(proxy, initialNode)}` : "新增代理"}</DialogTitle>
            <DialogDescription>配置服务器上的 VLESS Reality 代理。</DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-6 pb-4">
            <div className="grid grid-cols-1 gap-3">
              <Field label="代理名称 *">
                <Input autoFocus={!proxy} required maxLength={128} value={name} onChange={(event) => setName(event.target.value)} placeholder="Reality" />
              </Field>
              <Field label="协议 *">
                <Select value="vless">
                  <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                <SelectContent position="popper"><SelectItem value="vless">vless+reality</SelectItem></SelectContent>
                </Select>
              </Field>
            </div>

            <div className="flex flex-wrap items-start gap-x-3 gap-y-1 text-sm">
              <label className="inline-flex shrink-0 cursor-pointer items-center gap-2">
                <input type="checkbox" className="size-4 accent-primary" checked={includeNodeName} onChange={(event) => setIncludeNodeName(event.target.checked)} />
                附加节点地区前缀
              </label>
              <span className="text-muted-foreground">将自动生成：{name.trim() ? displayName : "代理名称"}</span>
            </div>

            <div className="grid grid-cols-1 gap-3">
              <Field label="所属节点 *">
                <Select value={String(nodeId || "")} onValueChange={changeNode} disabled={!nodes.length}>
                  <SelectTrigger className="w-full"><SelectValue placeholder="选择节点" /></SelectTrigger>
                  <SelectContent position="popper">
                    {nodes.map((node) => <SelectItem key={node.id} value={String(node.id)}>
                      {countryFlag(node.country) && <span aria-hidden="true">{countryFlag(node.country)}</span>}{node.name}
                    </SelectItem>)}
                  </SelectContent>
                </Select>
              </Field>
              <Field label="端口 *">
                <div className="flex gap-2"><Input required type="number" min="1" max="65535" step="1" value={port} onChange={(event) => setPort(event.target.value)} /><Button type="button" variant="outline" onClick={() => setPort(String(20000 + Math.floor(Math.random() * 45536)))}>随机</Button></div>
              </Field>
            </div>

            <section className="space-y-2 border-t pt-3">
              <Field label="连接地址 *" hint={addressType === "domain" ? undefined : "自动采用所属节点的可连接地址。"}>
                  <div className="mb-2 flex gap-1">
                    {([["ipv4", "IPv4"], ["ipv6", "IPv6"], ["domain", "自定义"]] as const).map(([type, label]) => <Button key={type} type="button" size="sm" variant={addressType === type ? "default" : "outline"} disabled={type !== "domain" && !nodeAddress(selectedNode, type)} onClick={() => changeAddressType(type)}>{label}</Button>)}
                  </div>
                  <div className="flex gap-2">
                    <Input
                      required
                      readOnly={addressType !== "domain"}
                      value={address}
                      onChange={(event) => setCustomAddress(event.target.value)}
                      placeholder={addressType === "domain" ? "hk.example.com 或 IP 地址" : "节点尚未上报此地址"}
                    />
                    <Badge variant="outline" className={`h-9 shrink-0 px-3 ${addressType === "domain" ? "" : "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"}`}>
                      {addressType === "domain" ? "自定义" : addressType.toUpperCase()}
                    </Badge>
                  </div>
              </Field>
            </section>

            <Card className="gap-0 overflow-hidden p-0">
              <button type="button" aria-expanded={advanced} className="flex w-full items-center gap-3 px-4 py-2.5 text-left hover:bg-muted/40" onClick={() => setAdvanced((open) => !open)}>
                <Settings2 className="size-4 shrink-0 text-muted-foreground" />
                <span className="min-w-0 flex-1">
                  <span className="block text-sm font-medium">高级配置</span>
                  <span className={`block text-xs ${completeReality ? "text-muted-foreground" : "text-amber-700 dark:text-amber-400"}`}>
                    {completeReality ? (proxy ? "SNI / DEST / Reality 密钥 · 配置完整" : "SNI / DEST / Reality 密钥 · 已自动生成") : "Reality 配置未完成"}
                  </span>
                </span>
                {!completeReality && <Badge variant="outline" className="border-amber-500/40 text-amber-700 dark:text-amber-400"><AlertTriangle /> 检查配置</Badge>}
                {advanced ? <ChevronUp className="size-4 shrink-0" /> : <ChevronDown className="size-4 shrink-0" />}
              </button>
              {advanced && (
                <div className="space-y-3 border-t p-4">
                  <h3 className="text-sm font-medium">Reality 伪装与 x25519 密钥</h3>
                  <div className="grid grid-cols-1 gap-3">
                  <Field label="SNI *"><Input required value={sni} onChange={(event) => setSni(event.target.value)} placeholder="www.amd.com" /></Field>
                  <Field label="DEST *"><Input required value={destination} onChange={(event) => setDestination(event.target.value)} placeholder="www.amd.com:443" /></Field>
                  </div>
                  <Field label="Public Key *">
                    <div className="flex gap-2"><Input className="min-w-0 flex-1 font-mono" required autoComplete="off" pattern="[A-Za-z0-9_-]{43}" value={publicKey} onChange={(event) => setPublicKey(event.target.value)} /><Button className="shrink-0" type="button" variant="outline" onClick={requestPairRegeneration}><RefreshCw /> 重新生成</Button><Button className="shrink-0" type="button" variant="outline" size="icon" title="复制公钥" aria-label="复制公钥" onClick={() => copyText(publicKey)}><Copy /></Button></div>
                  </Field>
                  <Field label="Private Key *">
                    <div className="flex gap-2"><Input className="min-w-0 flex-1 font-mono" required autoComplete="off" pattern="[A-Za-z0-9_-]{43}" type={showPrivateKey ? "text" : "password"} value={privateKey} onChange={(event) => setPrivateKey(event.target.value)} /><Button className="shrink-0" type="button" variant="outline" size="icon" title={showPrivateKey ? "隐藏私钥" : "显示私钥"} aria-label={showPrivateKey ? "隐藏私钥" : "显示私钥"} onClick={() => setShowPrivateKey((shown) => !shown)}>{showPrivateKey ? <EyeOff /> : <Eye />}</Button><Button className="shrink-0" type="button" variant="outline" size="icon" title="复制私钥" aria-label="复制私钥" onClick={() => copyText(privateKey)}><Copy /></Button></div>
                  </Field>
                  <Field label="Flow" hint="此代理下的用户共用该 Flow；客户端订阅和节点配置会同步使用。">
                    <Select value={flow} onValueChange={(value) => setFlow(value as Flow)}>
                      <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                      <SelectContent position="popper"><SelectItem value="xtls-rprx-vision">xtls-rprx-vision</SelectItem></SelectContent>
                    </Select>
                  </Field>
                  <Field label="Short ID *">
                    <div className="flex gap-2">
                      <Input className="min-w-0 flex-1" required maxLength={8} pattern="[0-9a-fA-F]{1,8}" value={shortId} onChange={(event) => setShortId(event.target.value)} placeholder="abcdef12" />
                      <Button className="shrink-0" type="button" variant="outline" onClick={() => setShortId(generateShortId())}><RefreshCw /> 重新生成</Button>
                    </div>
                  </Field>
                </div>
              )}
            </Card>
          </div>
          <DialogFooter className="shrink-0 border-t px-6 py-3">
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving || (!proxy && !nodes.length)}>{proxy ? "保存修改" : "创建代理"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
      {confirmKeyPair && <ConfirmDialog
        title="重新生成 Reality 密钥？"
        description="重新生成后，原有客户端使用的 Public Key 将失效，需要重新获取订阅或更新客户端配置。"
        confirmLabel="确认重新生成"
        onClose={() => setConfirmKeyPair(false)}
        onConfirm={regeneratePair}
      />}
    </Dialog>
  )
}

function ProxyPage({ nodes }: { nodes: Node[] }) {
  const { items, error, loading, reload } = useAllProxies()
  const [query, setQuery] = useState("")
  const [nodeFilter, setNodeFilter] = useState("all")
  const [statusFilter, setStatusFilter] = useState("all")
  const [creating, setCreating] = useState(false)
  const [editing, setEditing] = useState<Proxy | null>(null)
  const [deleting, setDeleting] = useState<Proxy | null>(null)
  const [removing, setRemoving] = useState(false)
  const [updatingProxyId, setUpdatingProxyId] = useState<number | null>(null)
  const drag = useDragOrder(items ?? [], "proxies", reload)
  const nodeById = new Map(nodes.map((node) => [node.id, node]))
  const visible = drag.order.filter((proxy) => {
    const node = nodeById.get(proxy.node_id)
    const needle = query.trim().toLowerCase()
    const matchesQuery = !needle || [displayProxyName(proxy, node), proxy.name, proxy.address, node?.name, String(proxy.port)].some((value) => value?.toLowerCase().includes(needle))
    const matchesNode = nodeFilter === "all" || String(proxy.node_id) === nodeFilter
    const matchesStatus = statusFilter === "all" || (statusFilter === "enabled" ? proxy.enabled : !proxy.enabled)
    return matchesQuery && matchesNode && matchesStatus
  })
  const searching = query.trim() !== "" || nodeFilter !== "all" || statusFilter !== "all"

  async function remove() {
    if (!deleting) return
    setRemoving(true)
    try {
      const result = await deleteProxy(deleting.id)
      notifySync("代理及其授权已删除", result.sync)
      setDeleting(null)
      reload()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setRemoving(false)
    }
  }

  async function setProxyEnabled(proxy: Proxy, enabled: boolean) {
    setUpdatingProxyId(proxy.id)
    try {
      const result = await updateProxy(proxy.id, proxyDraft(proxy, enabled))
      notifySync(enabled ? "代理已启用" : "代理已停用", result.sync)
      reload()
    } catch (error) {
      toast.error((error as Error).message)
    } finally {
      setUpdatingProxyId(null)
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-end gap-2">
        <div className="mr-auto flex w-full flex-wrap items-center gap-2 sm:w-auto">
          <AdminSearchInput value={query} onChange={setQuery} placeholder="搜索代理名称、地址或节点" />
          <Select value={nodeFilter} onValueChange={setNodeFilter}>
            <SelectTrigger className="w-36" aria-label="按节点筛选"><SelectValue /></SelectTrigger>
            <SelectContent position="popper">
              <SelectItem value="all">全部节点</SelectItem>
              {nodes.map((node) => <SelectItem key={node.id} value={String(node.id)}>{node.name}</SelectItem>)}
            </SelectContent>
          </Select>
          <StatusFilter value={statusFilter} onChange={setStatusFilter} labels={[["all", "全部状态"], ["enabled", "已启用"], ["disabled", "已停用"]]} />
        </div>
        <Button disabled={!nodes.length} onClick={() => setCreating(true)}><Plus /> 新增代理</Button>
      </div>

      <Card className="overflow-x-auto p-0">
        <Table className="table-auto min-w-max">
          <TableHeader className="bg-muted/50">
            <TableRow>
              <TableHead className="w-10 px-2" aria-label="排序" />
              <TableHead className="px-3">节点</TableHead>
              <TableHead className="px-3">名称</TableHead>
              <TableHead className="px-3">协议</TableHead>
              <TableHead className="px-3">地址</TableHead>
              <TableHead className="px-3">端口</TableHead>
              <TableHead className="w-28 max-w-28 px-3">SNI</TableHead>
              <TableHead className="px-3 text-center">状态</TableHead>
              <TableHead className="px-3 text-right">操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {visible.map((proxy) => {
              const node = nodeById.get(proxy.node_id)
              const proxyName = displayProxyName(proxy, node)
              const reality = proxy.config.reality
              return (
                <TableRow key={proxy.id} {...drag.row(proxy.id)} className="h-14">
                  <TableCell className="px-2">
                    <DragHandle {...drag.handle(proxy.id)} name={proxyName} disabled={searching} title={searching ? "清空搜索和筛选后可拖动排序" : undefined} />
                  </TableCell>
                  <TableCell className="px-3">
                    <div className="flex items-center gap-2">
                      <span aria-hidden="true" title={node?.online ? "在线" : "离线"} className={`size-2 shrink-0 rounded-full ${node?.online ? "bg-emerald-500" : "bg-muted-foreground/40"}`} />
                      <div>
                        <div className="font-medium">{node?.name ?? `节点 ${proxy.node_id}`}</div>
                        {node?.group && <div className="text-xs text-muted-foreground">{node.group}</div>}
                      </div>
                      {node?.country && <Badge variant="outline" title={node.country_pin ? "手动指定" : undefined} className="shrink-0 font-normal text-muted-foreground">{node.country}</Badge>}
                    </div>
                  </TableCell>
                  <TableCell className="px-3 font-medium">
                    <span className="min-w-0">{proxyName}</span>
                  </TableCell>
                  <TableCell className="px-3">
                    <ProxyProtocolBadge protocol={proxy.protocol} />
                  </TableCell>
                  <TableCell className="max-w-64 px-3">
                    <div className="flex min-w-0 items-center gap-2">
                      <ProxyAddressTypeBadge addressType={proxy.address_type} />
                      <span className="max-w-48 truncate text-sm" title={proxy.address}>{proxy.address}</span>
                    </div>
                  </TableCell>
                  <TableCell className="tnum px-3 text-sm">{proxy.port}</TableCell>
                  <TableCell className="w-28 max-w-28 px-3">
                    <TooltipText text={reality.server_name} className="w-24 max-w-24 text-sm" />
                  </TableCell>
                  <TableCell className="px-3 text-center">
                    <Switch checked={proxy.enabled} disabled={updatingProxyId !== null} onCheckedChange={(enabled) => setProxyEnabled(proxy, enabled)} aria-label={`${proxy.enabled ? "停用" : "启用"}代理 ${proxyName}`} />
                  </TableCell>
                  <TableCell className="whitespace-nowrap px-3 text-right">
                    <div className="flex items-center justify-end gap-1">
                      <Button variant="ghost" size="icon" className="max-md:size-8" title="编辑代理" aria-label="编辑代理" onClick={() => setEditing(proxy)}><Pencil /></Button>
                      <Button variant="ghost" size="icon" className="max-md:size-8 text-destructive hover:text-destructive" title="删除代理" aria-label="删除代理" onClick={() => setDeleting(proxy)}><Trash2 /></Button>
                    </div>
                  </TableCell>
                </TableRow>
              )
            })}
            {!loading && !error && !visible.length && (
              <TableRow><TableCell colSpan={8} className="py-10 text-center text-sm text-muted-foreground">
                {!items?.length ? (nodes.length ? "还没有代理，右上角新增" : "先添加节点，再创建代理") : "没有匹配的代理"}
              </TableCell></TableRow>
            )}
            {loading && <TableRow><TableCell colSpan={8} className="p-4"><Skeleton className="h-10 w-full" /></TableCell></TableRow>}
            {error && <TableRow><TableCell colSpan={8} className="py-8 text-center text-sm text-destructive">
              <div role="alert">加载代理失败：{error}</div>
              <Button variant="outline" size="sm" className="mt-3" onClick={reload}>重试</Button>
            </TableCell></TableRow>}
          </TableBody>
        </Table>
      </Card>

      {creating && <ProxyForm nodes={nodes} proxy={null} onClose={() => setCreating(false)} onSaved={reload} />}
      {editing && <ProxyForm nodes={nodes} proxy={editing} onClose={() => setEditing(null)} onSaved={reload} />}
      {deleting && <ConfirmDialog
        title={`删除代理「${displayProxyName(deleting, nodeById.get(deleting.node_id))}」？`}
        description="删除会一并解除所有关联用户授权，并自动同步对应节点的 sing-box 配置。"
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
  if (timestamp === null) return "永久有效"
  const date = new Date(timestamp * 1000 - 1000)
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`
}

function expired(user: User): boolean {
  return user.expires_at !== null && user.expires_at * 1000 <= Date.now()
}

function UserForm({ user, onClose, onSaved }: {
  user: User | null
  onClose: () => void
  onSaved: () => void
}) {
  const [username, setUsername] = useState(user?.username ?? "")
  const [password, setPassword] = useState("")
  const [trafficLimit, setTrafficLimit] = useState(user?.traffic_limit ? String(user.traffic_limit / 1024 ** 3) : "")
  const [deviceLimit, setDeviceLimit] = useState(user?.device_limit ? String(user.device_limit) : "")
  const [resetDay, setResetDay] = useState(user?.traffic_reset_day ? String(user.traffic_reset_day) : "")
  const [expires, setExpires] = useState(localDate(user?.expires_at ?? null))
  const [saving, setSaving] = useState(false)
  const [confirmUuid, setConfirmUuid] = useState(false)
  const uuidBusy = useRef(false)
  const [resettingUuid, setResettingUuid] = useState(false)
  const [uuid, setUuid] = useState(user?.uuid ?? "")

  async function save(event: React.FormEvent) {
    event.preventDefault()
    if (!username.trim()) return toast.error("请填写用户名")
    if (!user && !password) return toast.error("请设置用户登录密码")
    let day: number
    let limits: ReturnType<typeof parseUserLimits>
    try {
      limits = parseUserLimits(trafficLimit, deviceLimit)
      day = parseResetDay(resetDay)
    } catch (error) {
      return toast.error((error as Error).message)
    }
    const draft: UserDraft = {
      username: username.trim(),
      ...(password ? { password } : {}),
      enabled: user?.enabled ?? true,
      expires_at: localExpiryBoundary(expires),
      ...limits,
      traffic_reset_day: day,
    }
    setSaving(true)
    try {
      const saved = user ? await updateUser(user.id, draft) : await createUser(draft)
      notifySync(user ? "用户信息已更新" : "用户已创建", saved.sync)
      onClose()
      onSaved()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setSaving(false)
    }
  }

  async function resetUuid() {
    if (!user || uuidBusy.current) return
    uuidBusy.current = true
    setResettingUuid(true)
    try {
      const result = await resetUserUuid(user.id)
      setUuid(result.uuid)
      setConfirmUuid(false)
      onSaved()
      notifySync("UUID 已重置", result.sync)
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "UUID 重置失败")
    } finally {
      uuidBusy.current = false
      setResettingUuid(false)
    }
  }

  return (
    <>
    <Dialog open onOpenChange={(open) => !open && !uuidBusy.current && onClose()}>
      <DialogContent onOpenAutoFocus={(e) => e.preventDefault()} className="flex max-h-[85vh] w-full max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl">
        <form className="flex min-h-0 flex-1 flex-col" onSubmit={save}>
          <DialogHeader className="shrink-0 px-6 pt-6 pb-4">
            <DialogTitle className="pr-6 [overflow-wrap:anywhere]">{user ? `编辑用户：${user.username}` : "新建用户"}</DialogTitle>
            <DialogDescription>{user ? "修改用户信息、限额和有效期限。" : "创建代理用户，并设置限额和有效期限。"}</DialogDescription>
          </DialogHeader>
          <div className="grid min-h-0 flex-1 grid-cols-1 gap-5 overflow-y-auto px-6 pb-5 sm:grid-cols-2">
          <Field label="用户名 *">
            <Input autoFocus={!user} required maxLength={128} placeholder="如：client_01" value={username} readOnly={user?.username === "admin"} onChange={(e) => setUsername(e.target.value)} />
            {user?.username === "admin" && <p className="text-xs text-muted-foreground">默认用户名称固定为 admin。</p>}
          </Field>

          <Field label={user ? "重置登录密码（可选）" : "登录密码 *"} hint={user ? "留空则保持当前密码不变。" : "用户使用用户名和密码登录用户中心。"}>
            <Input
              type="password"
              autoComplete="new-password"
              required={!user}
              maxLength={1024}
              placeholder={user ? "留空保持不变" : "设置用户登录密码"}
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </Field>

          <Field label="流量限额（GiB）" hint="留空或填写 0 表示不限。">
            <Input type="number" min="0" step="any" aria-label="流量限额（GiB）" placeholder="不限" value={trafficLimit} onChange={(e) => setTrafficLimit(e.target.value)} />
          </Field>

          <Field label="设备数" hint="留空或填写 0 表示不限。">
            <Input type="number" min="0" step="1" aria-label="设备数" placeholder="不限" value={deviceLimit} onChange={(e) => setDeviceLimit(e.target.value)} />
          </Field>

          <Field label="流量重置日" hint="输入 1–31，留空不自动重置；短月份取月末，下次重置日生效。">
            <Input type="number" min="1" max="31" step="1" aria-label="流量重置日" placeholder="不自动重置" value={resetDay} onChange={(e) => setResetDay(e.target.value)} />
          </Field>

          <Field label="有效期限" hint="留空则永久有效。">
            <Input type="date" aria-label="有效期限" value={expires} onChange={(e) => setExpires(e.target.value)} />
            {expires && <p className="text-xs text-muted-foreground">所选日期当天结束时到期，按当前浏览器时区计算。</p>}
          </Field>

          {user && (
            <Field className="sm:col-span-2" label="UUID" hint="UUID 只用于代理认证；密码、资料和授权变更不会修改它。">
              <div className="flex flex-wrap items-center gap-3 rounded-md border bg-muted/30 px-3 py-2">
                <code className="min-w-0 flex-1 break-all font-mono text-sm select-all">{uuid}</code>
                <div className="flex flex-wrap items-center gap-2">
                  <Button type="button" variant="ghost" size="icon" className="size-7" title="复制 UUID" aria-label="复制 UUID" disabled={!uuid} onClick={() => copyText(uuid)}><Copy className="size-3.5" /></Button>
                  <Button type="button" variant="ghost" size="icon" className="size-7" title="重置 UUID" aria-label="重置 UUID" disabled={resettingUuid || saving} onClick={() => setConfirmUuid(true)}><RefreshCw className="size-3.5" /></Button>
                </div>
              </div>
            </Field>
          )}

          {!user && (
            <p className="rounded-lg border bg-muted/30 px-4 py-3 text-sm text-muted-foreground sm:col-span-2">创建用户后，可在「订阅」中为用户配置代理授权。</p>
          )}

          </div>
          <DialogFooter className="shrink-0 border-t px-6 py-4">
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving || resettingUuid}>{user ? "保存修改" : "确认创建"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
    {confirmUuid && <ConfirmDialog title="确认重置 UUID？" description="原 UUID 将失效，使用该用户的客户端需要更新订阅后重新连接。" confirmLabel="重置 UUID" busy={resettingUuid} onClose={() => { if (!uuidBusy.current) setConfirmUuid(false) }} onConfirm={() => void resetUuid()} />}
    </>
  )
}

function UserTrafficDialog({ user, onClose, onChanged }: { user: User; onClose: () => void; onChanged: () => void }) {
  const [items, setItems] = useState<ProxyTrafficSummary[] | null>(null)
  const [error, setError] = useState("")
  const [revision, setRevision] = useState(0)
  const [confirmReset, setConfirmReset] = useState(false)
  const [resetting, setResetting] = useState(false)

  useEffect(() => {
    let active = true
    getProxyUserTraffic(user.id).then((next) => {
      if (active) { setItems(next); setError("") }
    }).catch((e: Error) => {
      if (active) { setItems(null); setError(e.message) }
    })
    return () => { active = false }
  }, [user.id, revision])

  async function reset() {
    setResetting(true)
    try {
      await resetProxyUserTraffic(user.id)
      toast.success(`已清空用户「${user.username}」在所有节点的业务流量。`)
      setConfirmReset(false)
      setRevision((value) => value + 1)
      onChanged()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setResetting(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>业务流量：{user.username}</DialogTitle>
          <DialogDescription>按主机节点汇总。清零会保留 Core counter baseline。</DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          {error && <p role="alert" className="text-sm text-destructive">读取流量失败：{error}</p>}
          <Card className="overflow-x-auto p-0">
            <Table>
              <TableHeader className="bg-muted/50"><TableRow>
                <TableHead>节点</TableHead><TableHead>上行</TableHead><TableHead>下行</TableHead><TableHead>最近采集</TableHead>
              </TableRow></TableHeader>
              <TableBody>
                {items?.map((item) => <TableRow key={item.node_id}>
                  <TableCell>{item.node_name}</TableCell>
                  <TableCell className="tnum">{bytes(item.uplink_bytes)}</TableCell>
                  <TableCell className="tnum">{bytes(item.downlink_bytes)}</TableCell>
                  <TableCell className="text-sm text-muted-foreground">{item.last_seen_at ? new Date(item.last_seen_at * 1000).toLocaleString() : "—"}</TableCell>
                </TableRow>)}
                {items?.length === 0 && <TableRow><TableCell colSpan={4} className="py-6 text-center text-sm text-muted-foreground">尚无已采集的流量</TableCell></TableRow>}
                {items === null && !error && <TableRow><TableCell colSpan={4} className="p-4"><Skeleton className="h-9 w-full" /></TableCell></TableRow>}
              </TableBody>
            </Table>
          </Card>
        </div>
        <DialogFooter className="border-t pt-4">
          <Button variant="ghost" onClick={onClose}>关闭</Button>
          <Button variant="destructive" onClick={() => setConfirmReset(true)}>清空该用户流量</Button>
        </DialogFooter>
      </DialogContent>
      {confirmReset && <ConfirmDialog
        title={`清空用户「${user.username}」的业务流量？`}
        description="会清空该用户在所有节点的累计量，但保留 Core counter baseline，下一次新增流量会从零累计。"
        confirmLabel="清空流量"
        busy={resetting}
        onClose={() => setConfirmReset(false)}
        onConfirm={() => void reset()}
      />}
    </Dialog>
  )
}

function UsersPage() {
  const { items, error, loading, reload: refreshUsers } = useSharedUsers()
  const resources = useAdminResources()
  const [trafficResult, setTrafficResult] = useState<{ revision: number; items: Awaited<ReturnType<typeof listProxyUserTraffic>> | null; error: string } | null>(null)
  const [query, setQuery] = useState("")
  const [status, setStatus] = useState("all")
  const [creating, setCreating] = useState(false)
  const [editing, setEditing] = useState<User | null>(null)
  const [deleting, setDeleting] = useState<User | null>(null)
  const [removing, setRemoving] = useState(false)
  const [updatingUserId, setUpdatingUserId] = useState<number | null>(null)
  const [trafficRevision, setTrafficRevision] = useState(0)
  const [trafficUser, setTrafficUser] = useState<User | null>(null)

  useEffect(() => {
    let active = true
    const load = () => listProxyUserTraffic().then((items) => {
      if (active) setTrafficResult({ revision: trafficRevision, items, error: "" })
    }).catch((e: Error) => {
      if (active) setTrafficResult({ revision: trafficRevision, items: null, error: e.message })
    })
    void load()
    const timer = window.setInterval(() => void load(), 5_000)
    return () => { active = false; window.clearInterval(timer) }
  }, [trafficRevision])

  const trafficItems = trafficResult?.revision === trafficRevision ? trafficResult.items : null
  const trafficError = trafficResult?.revision === trafficRevision ? trafficResult.error : ""
  const trafficByUser = new Map((trafficItems ?? []).map((item) => [item.user_id, item]))
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
      const result = await deleteUser(deleting.id)
      notifySync("用户及其授权已删除", result.sync)
      setDeleting(null)
      reload()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setRemoving(false)
    }
  }

  async function setUserEnabled(user: User, enabled: boolean) {
    setUpdatingUserId(user.id)
    try {
      const result = await updateUser(user.id, { username: user.username, enabled, expires_at: user.expires_at })
      notifySync("用户状态已更新", result.sync)
      reload()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setUpdatingUserId(null)
    }
  }

  function reload() {
    resources.invalidateAuthorizations()
    void resources.users.refresh(true)
    setTrafficRevision((value) => value + 1)
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-end gap-2">
        <div className="mr-auto flex w-full flex-wrap items-center gap-2 sm:w-auto">
          <AdminSearchInput value={query} onChange={setQuery} placeholder="搜索用户名" />
          <StatusFilter value={status} onChange={setStatus} labels={[["all", "全部状态"], ["enabled", "已启用"], ["disabled", "已停用"], ["expired", "已过期"]]} />
        </div>
        <Button onClick={() => setCreating(true)}><Plus /> 新增用户</Button>
      </div>
      {trafficError && <p role="alert" className="text-sm text-destructive">业务流量读取失败：{trafficError}</p>}
      <Card className="overflow-x-auto p-0">
        <Table>
          <TableHeader className="bg-muted/50"><TableRow>
            <TableHead className="w-[7%] px-4">ID</TableHead>
            <TableHead className="w-[19%] px-4">用户名</TableHead>
            <TableHead className="w-[26%] min-w-68 px-4">流量使用情况</TableHead>
            <TableHead className="w-[12%] px-4">设备</TableHead>
            <TableHead className="w-[14%] px-4">到期时间</TableHead>
            <TableHead className="w-[14%] px-4">账户状态</TableHead>
            <TableHead className="text-right">操作</TableHead>
          </TableRow></TableHeader>
          <TableBody>
            {visible.map((user) => (
              <TableRow key={user.id} className="h-14">
                <TableCell className="tnum px-4 text-sm text-muted-foreground">{user.id}</TableCell>
                <TableCell className="px-4">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-semibold">{user.username}</span>
                    <Badge variant={user.username === "admin" ? "default" : "secondary"} className="font-normal">
                      {user.username === "admin" ? "管理员" : "普通用户"}
                    </Badge>
                  </div>
                </TableCell>
                <TableCell className="min-w-68 px-4 py-2">
                  <TrafficUsage used={(trafficByUser.get(user.id)?.uplink_bytes ?? 0) + (trafficByUser.get(user.id)?.downlink_bytes ?? 0)} limit={user.traffic_limit} />
                </TableCell>
                <TableCell className="tnum px-4 text-sm" title="暂未统计在线设备数">— / {user.device_limit || "不限"}</TableCell>
                <TableCell className="tnum px-4 text-sm">{displayDate(user.expires_at)}</TableCell>
                <TableCell className="px-4">
                  <div className="flex flex-wrap items-center gap-2">
                    <Switch
                      checked={user.enabled}
                      disabled={updatingUserId !== null}
                      onCheckedChange={(enabled) => setUserEnabled(user, enabled)}
                      aria-label={`${user.enabled ? "停用" : "启用"}用户 ${user.username}`}
                    />
                    <span className={`text-sm font-medium ${user.enabled ? "text-green-600 dark:text-green-400" : "text-muted-foreground"}`}>{user.enabled ? "启用" : "停用"}</span>
                    {expired(user) && <Badge variant="destructive" className="font-normal">已过期</Badge>}
                  </div>
                </TableCell>
                <TableCell className="whitespace-nowrap text-right">
                  <div className="flex items-center justify-end gap-1">
                    <Tooltip><TooltipTrigger asChild><Button type="button" variant="ghost" size="icon" className="size-8" aria-label={`重置用户 ${user.username} 的流量`} onClick={() => setTrafficUser(user)}><RefreshCw /></Button></TooltipTrigger><TooltipContent>重置流量</TooltipContent></Tooltip>
                    <Button type="button" variant="ghost" size="icon" className="size-8" title="编辑用户" aria-label={`编辑用户 ${user.username}`} onClick={() => setEditing(user)}><Pencil /></Button>
                    <Button type="button" variant="ghost" size="icon" disabled={user.username === "admin"} title={user.username === "admin" ? "管理员不可删除" : "删除用户"} aria-label={user.username === "admin" ? "管理员不可删除" : `删除用户 ${user.username}`} className="size-8 text-destructive hover:text-destructive" onClick={() => setDeleting(user)}><Trash2 /></Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}
            {!loading && !error && !visible.length && <TableRow><TableCell colSpan={7} className="py-10 text-center text-sm text-muted-foreground">{items?.length ? "没有匹配的用户" : "还没有用户，右上角新增"}</TableCell></TableRow>}
            {loading && <TableRow><TableCell colSpan={7} className="p-4"><Skeleton className="h-10 w-full" /></TableCell></TableRow>}
            {error && <TableRow><TableCell colSpan={7} className="py-8 text-center text-sm text-destructive">
              <div role="alert">加载用户失败：{error}</div>
              <Button variant="outline" size="sm" className="mt-3" onClick={refreshUsers}>重试</Button>
            </TableCell></TableRow>}
          </TableBody>
        </Table>
      </Card>
      {creating && <UserForm user={null} onClose={() => setCreating(false)} onSaved={reload} />}
      {editing && <UserForm user={editing} onClose={() => setEditing(null)} onSaved={reload} />}
      {trafficUser && <UserTrafficDialog user={trafficUser} onClose={() => setTrafficUser(null)} onChanged={() => setTrafficRevision((value) => value + 1)} />}
      {deleting && <ConfirmDialog
        title={`删除用户「${deleting.username}」？`}
        description={`此用户的 ${deleting.proxy_count} 条代理授权也会删除，并自动同步相关节点。`}
        confirmLabel="删除用户"
        busy={removing}
        onClose={() => setDeleting(null)}
        onConfirm={remove}
      />}
    </div>
  )
}

function SubscriptionForm({ user, proxies, nodes, accesses, ready, onClose, onSaved }: {
  ready: boolean
  user: User
  proxies: Proxy[]
  nodes: Node[]
  accesses: UserProxyAuthorization[]
  onClose: () => void
  onSaved: () => void
}) {
  const [autoAuthorizeNewProxies, setAutoAuthorizeNewProxies] = useState(user.auto_authorize_new_proxies)
  const [proxyQuery, setProxyQuery] = useState("")
  const [expandedGroups, setExpandedGroups] = useState<Record<number, boolean>>({})
  const [selectedIds, setSelectedIds] = useState<Set<number>>(() => new Set(accesses.filter((item) => item.access.enabled).map((item) => item.proxy.id)))
  const [saving, setSaving] = useState(false)
  const savingRef = useRef(false)
  const groups = useMemo(() => groupProxiesByNode(proxies, nodes), [proxies, nodes])
  const selectedProxies = proxies.filter((proxy) => selectedIds.has(proxy.id))
  const query = proxyQuery.trim().toLocaleLowerCase()

  function setProxySelected(proxyId: number, checked: boolean) {
    setSelectedIds((current) => {
      const next = new Set(current)
      if (checked) next.add(proxyId)
      else next.delete(proxyId)
      return next
    })
  }

  function setGroupSelected(proxyIds: number[]) {
    const nextIds = toggleProxyGroup(selectedIds, proxyIds)
    setSelectedIds(new Set(nextIds))
  }

  async function save(event: React.FormEvent) {
    event.preventDefault()
    if (!ready || savingRef.current) return
    savingRef.current = true
    setSaving(true)
    try {
      const result = await replaceUserAuthorizations(
        user.id,
        selectedProxies.map((proxy) => ({ proxy_id: proxy.id, enabled: true })),
        autoAuthorizeNewProxies,
      )
      notifySync("订阅已更新", result.sync)
      onClose()
      onSaved()
    } catch (error) {
      toast.error(`订阅授权未能保存：${(error as Error).message}`)
    } finally {
      savingRef.current = false
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && !savingRef.current && onClose()}>
      <DialogContent onOpenAutoFocus={(event) => event.preventDefault()} className="flex h-[min(42rem,85dvh)] w-full max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-3xl">
        <form className="flex min-h-0 flex-1 flex-col" onSubmit={save}>
          <DialogHeader className="shrink-0 px-6 pt-6 pb-4">
            <DialogTitle className="pr-6 [overflow-wrap:anywhere]">{`编辑订阅：${user.username}`}</DialogTitle>
            <DialogDescription>勾选需要授权的节点；取消全部勾选并保存可清空授权。</DialogDescription>
          </DialogHeader>
          <div className="shrink-0 space-y-3 px-6 pb-4">
            <div className="flex flex-wrap items-center gap-2">
              <Button type="button" variant="outline" size="sm" disabled={!ready || saving} onClick={() => setSelectedIds(new Set(proxies.map((proxy) => proxy.id)))}>全选代理</Button>
              <Button type="button" variant="outline" size="sm" disabled={!ready || saving} onClick={() => setSelectedIds(new Set())}>清空选择</Button>
            </div>
            <label className="flex items-center justify-between gap-3">
              <span className="space-y-1"><span className="block text-sm font-medium">自动授权新增代理</span><span className="block text-xs text-muted-foreground">保存后新建的代理会自动加入此用户订阅，已有代理仍按勾选结果授权。</span></span>
              <Switch checked={autoAuthorizeNewProxies} disabled={!ready || saving} onCheckedChange={setAutoAuthorizeNewProxies} />
            </label>
            <AdminSearchInput value={proxyQuery} onChange={setProxyQuery} placeholder="搜索节点名称或所属服务器..." className="w-full" />
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-4">
            <div className="rounded-md border">
                {groups.map((group) => {
                  const proxyIds = group.proxies.map((proxy) => proxy.id)
                  const state = proxyGroupSelection(selectedIds, proxyIds)
                  const isExpanded = expandedGroups[group.nodeId] ?? true
                  const matching = group.proxies.filter((proxy) => {
                    const node = group.node
                    return !query || [proxy.name, displayProxyName(proxy, node), node?.name, proxy.address].some((value) => value?.toLocaleLowerCase().includes(query))
                  })
                  if (query && !matching.length) return null
                  return (
                    <div key={group.nodeId} className="border-b last:border-0">
                      <div className="flex min-h-11 items-center gap-2 px-3 hover:bg-muted/40">
                        <button type="button" className="rounded p-1 text-muted-foreground hover:bg-muted" aria-label={`${isExpanded ? "收起" : "展开"}${group.node?.name ?? "节点"}`} aria-expanded={isExpanded} onClick={() => setExpandedGroups((current) => ({ ...current, [group.nodeId]: !isExpanded }))}>
                          {isExpanded ? <ChevronDown className="size-4" /> : <ChevronRight className="size-4" />}
                        </button>
                        <input
                          type="checkbox"
                          className="size-4 shrink-0 accent-primary"
                          checked={state.checked}
                          ref={(element) => { if (element) element.indeterminate = state.indeterminate }}
                          onChange={() => setGroupSelected(proxyIds)}
                          aria-label={`选择服务器 ${group.node?.name ?? `节点 ${group.nodeId}`}`}
                        />
                        <button type="button" className="flex min-w-0 flex-1 items-center justify-between gap-3 text-left text-sm" onClick={() => setExpandedGroups((current) => ({ ...current, [group.nodeId]: !isExpanded }))}>
                          <span className="flex min-w-0 flex-wrap items-baseline gap-x-2">
                            <span className="min-w-0 [overflow-wrap:anywhere] font-medium">{group.node?.name ?? `节点 ${group.nodeId}`}</span>
                            {group.node?.group && <span className="min-w-0 text-xs text-muted-foreground [overflow-wrap:anywhere]">{group.node.group}</span>}
                            {group.node?.country && <Badge variant="outline" title={group.node.country_pin ? "手动指定" : undefined} className="font-normal text-muted-foreground">{group.node.country}</Badge>}
                          </span>
                          <span className="tnum shrink-0 text-xs text-muted-foreground">{state.selected} / {state.total}</span>
                        </button>
                      </div>
                      {isExpanded && matching.map((proxy) => {
                        const node = group.node
                        return (
                          <label key={proxy.id} className="flex cursor-pointer items-start gap-3 border-t px-4 py-2.5 pl-11 hover:bg-muted/30">
                            <input type="checkbox" className="mt-1 size-4 shrink-0 accent-primary" checked={selectedIds.has(proxy.id)} onChange={(event) => setProxySelected(proxy.id, event.target.checked)} />
                            <span className="min-w-0 flex-1">
                              <span className="flex min-w-0 flex-wrap items-center gap-2">
                                <span className="min-w-0 text-sm font-medium [overflow-wrap:anywhere]">{displayProxyName(proxy, node)}</span>
                                <ProxyProtocolBadge protocol={proxy.protocol} />
                              </span>
                              <span className="mt-1 flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
                                <ProxyAddressTypeBadge addressType={proxy.address_type} />
                                <span className="truncate" title={`${proxy.address}:${proxy.port}`}>{proxy.address}:{proxy.port}</span>
                              </span>
                            </span>
                          </label>
                        )
                      })}
                    </div>
                  )
                })}
                {!groups.length && <p className="p-4 text-center text-sm text-muted-foreground">目前没有可授权的代理节点</p>}
                {!!groups.length && query && !groups.some((group) => group.proxies.some((proxy) => [proxy.name, displayProxyName(proxy, group.node), group.node?.name, proxy.address].some((value) => value?.toLocaleLowerCase().includes(query)))) && <p className="p-4 text-center text-sm text-muted-foreground">没有匹配的代理节点</p>}
              </div>

          </div>
          <DialogFooter className="shrink-0 items-center border-t px-6 py-4">
            <span className="mr-auto text-sm text-muted-foreground">已选择 {selectedProxies.length} 个节点</span>
            <Button type="button" variant="ghost" disabled={saving} onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving || !ready}>{saving ? "保存中..." : "保存修改"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function SubscriptionProxyRows({ accesses, proxies, nodes }: { accesses: UserProxyAuthorization[]; proxies: Proxy[]; nodes: Node[] }) {
  const proxyById = new Map(proxies.map((proxy) => [proxy.id, proxy]))
  const nodeById = new Map(nodes.map((node) => [node.id, node]))
  const nodeOrder = new Map(nodes.map((node, index) => [node.id, index]))
  const ordered = accesses.slice().sort((a, b) => (nodeOrder.get(a.proxy.node_id) ?? Number.MAX_SAFE_INTEGER) - (nodeOrder.get(b.proxy.node_id) ?? Number.MAX_SAFE_INTEGER) || a.proxy.id - b.proxy.id)
  return (
    <Table className="table-auto min-w-max">
      <TableHeader className="bg-muted/30"><TableRow>
        <TableHead className="px-3">节点</TableHead>
        <TableHead className="px-3">名称</TableHead>
        <TableHead className="px-3">协议</TableHead>
        <TableHead className="px-3">地址</TableHead>
        <TableHead className="px-3">端口</TableHead>
        <TableHead className="w-28 max-w-28 px-3">SNI</TableHead>
      </TableRow></TableHeader>
      <TableBody>
        {ordered.map((access) => {
          const proxy = proxyById.get(access.proxy.id)
          const node = nodeById.get(access.proxy.node_id)
          const reality = proxy?.config.reality
          return <TableRow key={access.proxy.id} className="h-12">
            <TableCell className="px-3">
              <div>
                <div className="flex w-max flex-nowrap items-center gap-2 whitespace-nowrap font-medium">
                  <span>{node?.name ?? `节点 ${access.proxy.node_id}`}</span>
                  {node?.country && <Badge variant="outline" title={node.country_pin ? "手动指定" : undefined} className="font-normal text-muted-foreground">{node.country}</Badge>}
                </div>
                {node?.group && <div className="text-xs text-muted-foreground">{node.group}</div>}
              </div>
            </TableCell>
            <TableCell className="px-3 font-medium">{displayProxyName(access.proxy, node)}</TableCell>
            <TableCell className="px-3"><ProxyProtocolBadge protocol={access.proxy.protocol} /></TableCell>
            <TableCell className="max-w-64 px-3"><div className="flex min-w-0 items-center gap-2"><ProxyAddressTypeBadge addressType={access.proxy.address_type} /><span className="max-w-48 truncate text-sm" title={access.proxy.address}>{access.proxy.address}</span></div></TableCell>
            <TableCell className="tnum px-3 text-sm">{access.proxy.port}</TableCell>
            <TableCell className="w-28 max-w-28 px-3"><TooltipText text={reality?.server_name ?? "—"} className="w-24 max-w-24 text-sm" /></TableCell>
          </TableRow>
        })}
        {!ordered.length && <TableRow><TableCell colSpan={6} className="py-5 text-center text-sm text-muted-foreground">该用户还没有代理节点授权</TableCell></TableRow>}
      </TableBody>
    </Table>
  )
}

function SubscriptionsPage({ nodes, search }: { nodes: Node[]; search: string }) {
  const { items: proxies, error: proxyError, loading: proxiesLoading, reload: reloadProxies } = useAllProxies()
  const requestedId = new URLSearchParams(search).get("user_id")
  const requestedUserId = requestedId && /^\d+$/.test(requestedId) ? Number(requestedId) : null
  const { items: users, error: usersError, loading: usersLoading, reload: refreshUsers } = useSharedUsers()
  const resources = useAdminResources()
  const usersRevision = resources.authorizationRevision
  const [userQuery, setUserQuery] = useState("")
  const [status, setStatus] = useState("all")
  const [accessCache, setAccessCache] = useState<Map<number, UserProxyAuthorization[]>>(() => new Map())
  const accessCacheRef = useRef(accessCache)
  const accessGeneration = useRef(0)
  const editOpening = useRef(false)
  const accessRequests = useRef(new Map<number, Promise<UserProxyAuthorization[]>>())
  const [accessLoading, setAccessLoading] = useState<Set<number>>(() => new Set())
  const [accessErrors, setAccessErrors] = useState<Record<number, string>>({})
  const [expandedUsers, setExpandedUsers] = useState<Set<number>>(() => requestedUserId === null ? new Set() : new Set([requestedUserId]))
  const [editing, setEditing] = useState<User | null>(null)
  const [updatingUserId, setUpdatingUserId] = useState<number | null>(null)
  const needle = userQuery.trim().toLocaleLowerCase()
  const proxyById = useMemo(() => new Map((proxies ?? []).map((proxy) => [proxy.id, proxy])), [proxies])
  const nodeById = useMemo(() => new Map(nodes.map((node) => [node.id, node])), [nodes])
  const visibleUsers = (users ?? []).filter((user) => {
    const matchesStatus = status === "all" || (status === "enabled" ? user.enabled : !user.enabled)
    if (!matchesStatus) return false
    if (!needle || user.username.toLocaleLowerCase().includes(needle)) return true
    const accesses = accessCache.get(user.id)
    return accesses ? subscriptionSearchMatches(user.username, accesses, proxyById, nodeById, needle) : false
  })
  const matchingAccessesPending = Boolean(needle && users?.some((user) => !accessCache.has(user.id) && !accessErrors[user.id]))

  const loadAccesses = useCallback((userId: number): Promise<UserProxyAuthorization[]> => {
    const cached = accessCacheRef.current.get(userId)
    if (cached) return Promise.resolve(cached)
    const pending = accessRequests.current.get(userId)
    if (pending) return pending
    setAccessLoading((current) => new Set(current).add(userId))
    setAccessErrors((current) => { const next = { ...current }; delete next[userId]; return next })
    const generation = accessGeneration.current
    const request = listUserAuthorizations(userId).then((items) => {
      if (generation !== accessGeneration.current) return items
      const next = new Map(accessCacheRef.current)
      next.set(userId, items)
      accessCacheRef.current = next
      setAccessCache(next)
      return items
    }).catch((error: Error) => {
      if (generation !== accessGeneration.current) throw error
      setAccessErrors((current) => ({ ...current, [userId]: error.message }))
      throw error
    }).finally(() => {
      if (generation !== accessGeneration.current) return
      accessRequests.current.delete(userId)
      setAccessLoading((current) => { const next = new Set(current); next.delete(userId); return next })
    })
    accessRequests.current.set(userId, request)
    return request
  }, [])

  useEffect(() => {
    const requests = accessRequests.current
    accessGeneration.current += 1
    requests.clear()
    // Synchronize the page-local authorization cache with shared invalidations.
    // eslint-disable-next-line react/set-state-in-effect
    setAccessLoading(new Set())
    accessCacheRef.current = new Map()
    setAccessCache(new Map())
    setAccessErrors({})
    return () => {
      accessGeneration.current += 1
      requests.clear()
    }
  }, [usersRevision])

  useEffect(() => {
    if (requestedUserId !== null && users?.some((user) => user.id === requestedUserId)) {
      void loadAccesses(requestedUserId).catch(() => {})
    }
  }, [requestedUserId, users, usersRevision, loadAccesses])

  useEffect(() => {
    if (!users) return
    for (const id of expandedUsers) {
      if (users.some((user) => user.id === id)) void loadAccesses(id).catch(() => {})
    }
  }, [users, expandedUsers, usersRevision, loadAccesses])

  const userKey = (users ?? []).map((user) => user.id).join(",")
  useEffect(() => {
    if (!needle || !userKey) return
    let active = true
    const ids = userKey.split(",").map(Number)
    void (async () => {
      for (let index = 0; index < ids.length && active; index += 8) {
        await Promise.all(ids.slice(index, index + 8).map((id) => loadAccesses(id).catch(() => [])))
      }
    })()
    return () => { active = false }
  }, [needle, userKey, usersRevision, loadAccesses])

  async function toggleExpanded(user: User) {
    const isOpen = expandedUsers.has(user.id)
    setExpandedUsers((current) => { const next = new Set(current); if (isOpen) next.delete(user.id); else next.add(user.id); return next })
    if (!isOpen) void loadAccesses(user.id).catch(() => {})
  }

  async function openEdit(user: User) {
    if (editOpening.current || proxiesLoading || proxyError || proxies === null) return
    editOpening.current = true
    try {
      await loadAccesses(user.id)
      setEditing(user)
    } catch (error) {
      toast.error(`加载 ${user.username} 的授权失败：${(error as Error).message}`)
    } finally {
      editOpening.current = false
    }
  }

  async function setUserEnabled(user: User, enabled: boolean) {
    setUpdatingUserId(user.id)
    try {
      const result = await updateUser(user.id, { username: user.username, enabled, expires_at: user.expires_at })
      notifySync(enabled ? "订阅授权已启用" : "订阅授权已停用", result.sync)
      void resources.users.refresh(true)
    } catch (error) {
      toast.error((error as Error).message)
    } finally {
      setUpdatingUserId(null)
    }
  }

  function reloadAll() {
    resources.invalidateAuthorizations()
    void resources.users.refresh(true)
  }

  return (
    <TooltipProvider>
      <div className="space-y-4">
        <div className="flex flex-wrap items-center justify-end gap-2">
          <div className="mr-auto flex w-full flex-wrap items-center gap-2 sm:w-auto">
            <AdminSearchInput value={userQuery} onChange={setUserQuery} placeholder="搜索用户或节点名称" />
            <Select value={status} onValueChange={setStatus}>
              <SelectTrigger className="w-32" aria-label="按订阅状态筛选"><SelectValue /></SelectTrigger>
              <SelectContent position="popper"><SelectItem value="all">全部状态</SelectItem><SelectItem value="enabled">已启用</SelectItem><SelectItem value="disabled">已禁用</SelectItem></SelectContent>
            </Select>
          </div>
        </div>

        <Card className="overflow-x-auto p-0">
          <Table>
            <TableHeader className="bg-muted/50"><TableRow>
              <TableHead className="w-[45%] px-3">用户</TableHead>
              <TableHead className="w-[40%] px-3">授权节点</TableHead>
              <TableHead className="w-[15%] px-3 text-center">状态</TableHead>
            </TableRow></TableHeader>
            <TableBody>
              {visibleUsers.map((user) => {
                const isExpanded = expandedUsers.has(user.id)
                const accesses = accessCache.get(user.id)
                const rowError = accessErrors[user.id]
                return <Fragment key={user.id}>
                  <TableRow className="h-14">
                    <TableCell className="max-w-0 px-3">
                      <div className="flex min-w-0 items-center gap-2">
                        <Button type="button" variant="ghost" size="icon-sm" className="size-7" aria-label={`${isExpanded ? "收起" : "展开"}${user.username}的节点`} aria-expanded={isExpanded} onClick={() => void toggleExpanded(user)}>
                          {isExpanded ? <ChevronDown /> : <ChevronRight />}
                        </Button>
                        <span className="truncate font-semibold">{user.username}</span>
                      </div>
                    </TableCell>
                    <TableCell className="px-3 text-sm"><Button type="button" variant="link" className="h-auto p-0" disabled={accessLoading.has(user.id) || proxiesLoading || !!proxyError} aria-label={`编辑 ${user.username} 的授权节点`} onClick={() => void openEdit(user)}>{accessLoading.has(user.id) ? "加载中..." : `${user.proxy_count} 个节点`}</Button></TableCell>
                    <TableCell className="px-3 text-center"><Switch checked={user.enabled} disabled={updatingUserId !== null} onCheckedChange={(enabled) => void setUserEnabled(user, enabled)} aria-label={`${user.enabled ? "停用" : "启用"}${user.username}的订阅授权`} /></TableCell>
                  </TableRow>
                  {isExpanded && <TableRow className="bg-muted/20 hover:bg-muted/20"><TableCell colSpan={3} className="p-0">
                    {rowError ? <div className="p-4 text-sm text-destructive" role="alert">加载授权节点失败：{rowError}<Button variant="link" className="h-auto p-0 pl-2" onClick={() => void loadAccesses(user.id).catch(() => {})}>重试</Button></div>
                      : accessLoading.has(user.id) || proxiesLoading ? <div className="p-4"><Skeleton className="h-9 w-full" /></div>
                      : accesses ? <SubscriptionProxyRows accesses={accesses} proxies={proxies ?? []} nodes={nodes} /> : null}
                  </TableCell></TableRow>}
                </Fragment>
              })}
              {!usersLoading && !usersError && !visibleUsers.length && <TableRow><TableCell colSpan={3} className="py-10 text-center text-sm text-muted-foreground">{matchingAccessesPending ? "正在搜索授权节点..." : users?.length ? "没有匹配的用户或节点" : "还没有用户"}</TableCell></TableRow>}
              {usersLoading && <TableRow><TableCell colSpan={3} className="p-4"><Skeleton className="h-10 w-full" /></TableCell></TableRow>}
              {usersError && <TableRow><TableCell colSpan={3} className="py-8 text-center text-sm text-destructive"><div role="alert">加载用户失败：{usersError}</div><Button variant="outline" size="sm" className="mt-3" onClick={refreshUsers}>重试</Button></TableCell></TableRow>}
              {proxyError && <TableRow><TableCell colSpan={3} className="py-8 text-center text-sm text-destructive"><div role="alert">加载代理失败：{proxyError}</div><Button variant="outline" size="sm" className="mt-3" onClick={reloadProxies}>重试</Button></TableCell></TableRow>}
            </TableBody>
          </Table>
        </Card>

        {editing && <SubscriptionForm key={`edit-${editing.id}`} user={editing} proxies={proxies ?? []} nodes={nodes} accesses={accessCache.get(editing.id) ?? []} ready={!proxiesLoading && !proxyError && proxies !== null} onClose={() => setEditing(null)} onSaved={reloadAll} />}
      </div>
    </TooltipProvider>
  )
}

export function ProxiesPage({ nodes }: { nodes: Node[] }) {
  return <ProxyPage nodes={nodes} />
}

export function UsersResourcePage() {
  return <UsersPage />
}

export function SubscriptionsResourcePage({ nodes, search }: { nodes: Node[]; search: string; go: Go }) {
  return <SubscriptionsPage nodes={nodes} search={search} />
}
