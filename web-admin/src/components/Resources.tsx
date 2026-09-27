import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { AlertTriangle, ChevronDown, ChevronRight, ChevronUp, Copy, Eye, EyeOff, KeyRound, Pencil, Plus, RefreshCw, Settings2, Trash2 } from "lucide-react"
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
import { Skeleton } from "@/components/ui/skeleton"
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip"
import { AdminConfirmDialog as ConfirmDialog, AdminSearchInput } from "@/components/AdminShared"
import type { Node } from "@/lib/api"
import { bytes } from "@/lib/format"
import { proxyDraft } from "@/lib/proxy-draft"
import { countryFlag, displayProxyName } from "@/lib/proxy-name"
import { ProxyAddressTypeBadge, ProxyProtocolBadge } from "@/components/ProxyBadges"
import { generateRealityKeyPair, generateShortId, realityKeyPairMatches } from "@/lib/reality"
import { formatSni, parseSni } from "@/lib/sni"
import { existingAuthorizationSettings, groupProxiesByNode, proxyGroupSelection, subscriptionSearchMatches, toggleProxyGroup } from "@/lib/subscription"
import {
  createProxy,
  createUser,
  deleteProxy,
  deleteUser,
  deleteUserProxy,
  listAllProxies,
  getProxyUserTraffic,
  listProxyNodeTraffic,
  listProxyUserTraffic,
  listUserAuthorizations,
  listUsers,
  saveUserProxy,
  resetUserUuid,
  resetProxyNodeTraffic,
  resetProxyUserTraffic,
  updateProxy,
  updateUser,
  type Flow,
  type Proxy,
  type ProxyDraft,
  type User,
  type UserDraft,
  type UserProxyAuthorization,
  type ProxyTrafficSummary,
} from "@/lib/resources"

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
  const [sni, setSni] = useState(initialReality ? formatSni(initialReality.server_name, initialReality.server_port) : "www.amd.com:443")
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
  const completeReality = Boolean(parsedSni && realityKeyPairMatches(privateKey.trim(), publicKey.trim())
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
    try {
      parsedSniValue = parseSni(sni)
    } catch (error) {
      setAdvanced(true)
      return toast.error((error as Error).message)
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
      config: {
        reality: { ...currentReality, ...parsedSniValue },
      },
    }

    setSaving(true)
    try {
      if (proxy) await updateProxy(proxy.id, draft, nodeId)
      else await createProxy(nodeId, draft)
      toast.success(proxy
        ? "代理配置已更新，变更将在该节点下次应用配置后生效。"
        : "代理已创建，变更将在该节点下次应用配置后生效。")
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
      <DialogContent onOpenAutoFocus={(event) => event.preventDefault()} className="flex max-h-[90vh] w-full max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-[960px]">
        <form className="flex min-h-0 flex-1 flex-col" onSubmit={save}>
          <DialogHeader className="shrink-0 px-6 pt-5 pb-3">
            <DialogTitle>{proxy ? `编辑代理：${displayProxyName(proxy, initialNode)}` : "新增代理"}</DialogTitle>
            <DialogDescription>配置服务器上的 VLESS Reality 代理。</DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-6 pb-4">
            <div className="grid gap-3 sm:grid-cols-[minmax(0,3fr)_minmax(12rem,2fr)]">
              <Field label="代理名称 *">
                <Input autoFocus={!proxy} required maxLength={128} value={name} onChange={(event) => setName(event.target.value)} placeholder="Reality" />
              </Field>
              <Field label="协议 *">
                <Select value="vless">
                  <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                  <SelectContent position="popper"><SelectItem value="vless">VLESS + Reality</SelectItem></SelectContent>
                </Select>
              </Field>
            </div>

            <div className="flex flex-wrap items-start gap-x-3 gap-y-1 text-sm">
              <label className="inline-flex shrink-0 cursor-pointer items-center gap-2">
                <input type="checkbox" className="size-4 accent-primary" checked={includeNodeName} onChange={(event) => setIncludeNodeName(event.target.checked)} />
                名称附带节点名称
              </label>
              <span className="text-muted-foreground">将自动生成：{name.trim() ? displayName : "代理名称"}</span>
            </div>

            <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_10rem]">
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
                <Input required type="number" min="1" max="65535" step="1" value={port} onChange={(event) => setPort(event.target.value)} />
              </Field>
            </div>

            <section className="space-y-2 border-t pt-3">
              <h3 className="text-sm font-medium">连接地址</h3>
              <div className="grid gap-3 sm:grid-cols-[13rem_minmax(0,1fr)]">
                <Field label="地址类型 *">
                  <Select value={addressType} onValueChange={(value) => changeAddressType(value as ProxyDraft["address_type"])}>
                    <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                    <SelectContent position="popper">
                      <SelectItem value="ipv4" disabled={!nodeAddress(selectedNode, "ipv4")}>IPv4{!nodeAddress(selectedNode, "ipv4") && "（不可用）"}</SelectItem>
                      <SelectItem value="ipv6" disabled={!nodeAddress(selectedNode, "ipv6")}>IPv6{!nodeAddress(selectedNode, "ipv6") && "（不可用）"}</SelectItem>
                      <SelectItem value="domain">自定义</SelectItem>
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="连接地址 *" hint={addressType === "domain" ? undefined : "自动采用所属节点的可连接地址。"}>
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
              </div>
            </section>

            <Card className="gap-0 overflow-hidden p-0">
              <button type="button" aria-expanded={advanced} className="flex w-full items-center gap-3 px-4 py-2.5 text-left hover:bg-muted/40" onClick={() => setAdvanced((open) => !open)}>
                <Settings2 className="size-4 shrink-0 text-muted-foreground" />
                <span className="min-w-0 flex-1">
                  <span className="block text-sm font-medium">高级配置</span>
                  <span className={`block text-xs ${completeReality ? "text-muted-foreground" : "text-amber-700 dark:text-amber-400"}`}>
                    {completeReality ? (proxy ? "SNI / Reality 密钥 · 配置完整" : "SNI / Reality 密钥 · 已自动生成") : "Reality 配置未完成"}
                  </span>
                </span>
                {!completeReality && <Badge variant="outline" className="border-amber-500/40 text-amber-700 dark:text-amber-400"><AlertTriangle /> 检查配置</Badge>}
                {advanced ? <ChevronUp className="size-4 shrink-0" /> : <ChevronDown className="size-4 shrink-0" />}
              </button>
              {advanced && (
                <div className="space-y-3 border-t p-4">
                  <Field label="SNI *">
                    <Input required value={sni} onChange={(event) => setSni(event.target.value)} placeholder="www.amd.com:443" />
                  </Field>
                  <Field label="Short ID *">
                    <div className="flex gap-2">
                      <Input className="min-w-0 flex-1" required maxLength={8} pattern="[0-9a-fA-F]{1,8}" value={shortId} onChange={(event) => setShortId(event.target.value)} placeholder="abcdef12" />
                      <Button className="shrink-0" type="button" variant="outline" onClick={() => setShortId(generateShortId())}><RefreshCw /> 重新生成</Button>
                    </div>
                  </Field>
                  <Field label="Private Key *">
                    <div className="flex gap-2">
                      <Input className="min-w-0 flex-1 font-mono" required autoComplete="off" pattern="[A-Za-z0-9_-]{43}" type={showPrivateKey ? "text" : "password"} value={privateKey} onChange={(event) => setPrivateKey(event.target.value)} />
                      <Button className="shrink-0" type="button" variant="outline" onClick={requestPairRegeneration}><RefreshCw /> 重新生成</Button>
                      <Button className="shrink-0" type="button" variant="outline" size="icon" title={showPrivateKey ? "隐藏私钥" : "显示私钥"} aria-label={showPrivateKey ? "隐藏私钥" : "显示私钥"} onClick={() => setShowPrivateKey((shown) => !shown)}>{showPrivateKey ? <EyeOff /> : <Eye />}</Button>
                      <Button className="shrink-0" type="button" variant="outline" size="icon" title="复制私钥" aria-label="复制私钥" onClick={() => copyText(privateKey)}><Copy /></Button>
                    </div>
                  </Field>
                  <Field label="Public Key *">
                    <div className="flex gap-2">
                      <Input className="min-w-0 flex-1 font-mono" required autoComplete="off" pattern="[A-Za-z0-9_-]{43}" value={publicKey} onChange={(event) => setPublicKey(event.target.value)} />
                      <Button className="shrink-0" type="button" variant="outline" onClick={requestPairRegeneration}><RefreshCw /> 重新生成</Button>
                      <Button className="shrink-0" type="button" variant="outline" size="icon" title="复制公钥" aria-label="复制公钥" onClick={() => copyText(publicKey)}><Copy /></Button>
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
  const { items, error, loading, reload } = useAllProxies(nodes)
  const [trafficItems, setTrafficItems] = useState<ProxyTrafficSummary[] | null>(null)
  const [trafficError, setTrafficError] = useState("")
  const [trafficRevision, setTrafficRevision] = useState(0)
  const [trafficResetNode, setTrafficResetNode] = useState<ProxyTrafficSummary | null>(null)
  const [resettingTraffic, setResettingTraffic] = useState(false)
  const [query, setQuery] = useState("")
  const [nodeFilter, setNodeFilter] = useState("all")
  const [statusFilter, setStatusFilter] = useState("all")
  const [creating, setCreating] = useState(false)
  const [editing, setEditing] = useState<Proxy | null>(null)
  const [deleting, setDeleting] = useState<Proxy | null>(null)
  const [removing, setRemoving] = useState(false)
  const [updatingProxyId, setUpdatingProxyId] = useState<number | null>(null)
  useEffect(() => {
    let active = true
    const load = () => listProxyNodeTraffic().then((next) => {
      if (active) { setTrafficItems(next); setTrafficError("") }
    }).catch((e: Error) => {
      if (active) { setTrafficItems(null); setTrafficError(e.message) }
    })
    void load()
    const timer = window.setInterval(() => void load(), 5_000)
    return () => { active = false; window.clearInterval(timer) }
  }, [trafficRevision])
  const nodeById = new Map(nodes.map((node) => [node.id, node]))
  const nodeOrder = new Map(nodes.map((node, index) => [node.id, index]))
  const visible = (items ?? []).filter((proxy) => {
    const node = nodeById.get(proxy.node_id)
    const needle = query.trim().toLowerCase()
    const matchesQuery = !needle || [displayProxyName(proxy, node), proxy.name, proxy.address, node?.name, String(proxy.port)].some((value) => value?.toLowerCase().includes(needle))
    const matchesNode = nodeFilter === "all" || String(proxy.node_id) === nodeFilter
    const matchesStatus = statusFilter === "all" || (statusFilter === "enabled" ? proxy.enabled : !proxy.enabled)
    return matchesQuery && matchesNode && matchesStatus
  }).sort((a, b) => (nodeOrder.get(a.node_id) ?? Number.MAX_SAFE_INTEGER) - (nodeOrder.get(b.node_id) ?? Number.MAX_SAFE_INTEGER) || a.id - b.id)

  async function remove() {
    if (!deleting) return
    setRemoving(true)
    try {
      await deleteProxy(deleting.id)
      toast.success("代理及其授权已删除，变更将在对应节点下次应用配置后生效。")
      setDeleting(null)
      reload()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setRemoving(false)
    }
  }

  async function resetNodeTraffic() {
    if (!trafficResetNode) return
    setResettingTraffic(true)
    try {
      await resetProxyNodeTraffic(trafficResetNode.node_id)
      toast.success(`已清空节点「${trafficResetNode.node_name}」的业务流量。`)
      setTrafficResetNode(null)
      setTrafficRevision((value) => value + 1)
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setResettingTraffic(false)
    }
  }

  async function setProxyEnabled(proxy: Proxy, enabled: boolean) {
    setUpdatingProxyId(proxy.id)
    try {
      await updateProxy(proxy.id, proxyDraft(proxy, enabled))
      toast.success(enabled
        ? "代理已启用，变更将在该节点下次应用配置后生效。"
        : "代理已停用，变更将在该节点下次应用配置后生效。")
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

      {trafficError && <p role="alert" className="text-sm text-destructive">节点业务流量读取失败：{trafficError}</p>}
      {trafficItems && trafficItems.length > 0 && <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {trafficItems.map((traffic) => <Card key={traffic.node_id} className="flex flex-row items-center justify-between gap-4 py-3">
          <div className="min-w-0">
            <div className="truncate font-medium">{traffic.node_name}</div>
            <div className="mt-1 text-xs text-muted-foreground">上行 {bytes(traffic.uplink_bytes)} · 下行 {bytes(traffic.downlink_bytes)}</div>
            <div className="mt-1 text-xs text-muted-foreground">最近采集：{traffic.last_seen_at ? new Date(traffic.last_seen_at * 1000).toLocaleString() : "尚未采集"}</div>
          </div>
          <Button variant="outline" size="sm" onClick={() => setTrafficResetNode(traffic)}>清零</Button>
        </Card>)}
      </div>}

      <Card className="overflow-x-auto p-0">
        <Table className="table-fixed">
          <TableHeader className="bg-muted/50">
            <TableRow>
              <TableHead className="w-[12%] max-md:w-[9%] px-1 md:px-3">节点</TableHead>
              <TableHead className="w-[12%] max-md:w-[11%] px-1 md:px-3">名称</TableHead>
              <TableHead className="hidden w-[16%] px-3 md:table-cell">协议</TableHead>
              <TableHead className="hidden w-[16%] px-3 md:table-cell">地址</TableHead>
              <TableHead className="w-[8%] max-md:w-[14%] px-1 md:px-3">端口</TableHead>
              <TableHead className="w-[22%] max-md:w-[28%] px-1 md:px-3">SNI</TableHead>
              <TableHead className="w-[6%] max-md:w-[12%] px-0 md:px-3 text-center">状态</TableHead>
              <TableHead className="w-[8%] max-md:w-[26%] px-1 md:px-3 text-right">操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {visible.map((proxy) => {
              const node = nodeById.get(proxy.node_id)
              const proxyName = displayProxyName(proxy, node)
              const reality = proxy.config.reality
              return (
                <TableRow key={proxy.id} className="h-14">
                  <TableCell className="px-1 md:px-3">
                    <div className="flex min-w-0 items-center gap-2">
                      <span aria-hidden="true" title={node?.online ? "在线" : "离线"} className={`size-2 shrink-0 rounded-full ${node?.online ? "bg-emerald-500" : "bg-muted-foreground/40"}`} />
                      <span className="truncate font-medium">{node?.name ?? `节点 ${proxy.node_id}`}</span>
                    </div>
                  </TableCell>
                  <TableCell className="max-w-0 px-1 md:px-3">
                    <div className="truncate font-medium" title={proxyName}>{proxyName}</div>
                  </TableCell>
                  <TableCell className="hidden px-3 md:table-cell">
                    <ProxyProtocolBadge protocol={proxy.protocol} />
                  </TableCell>
                  <TableCell className="hidden max-w-0 px-3 md:table-cell">
                    <div className="flex min-w-0 items-center gap-2">
                      <ProxyAddressTypeBadge addressType={proxy.address_type} />
                      <span className="truncate text-sm" title={proxy.address}>{proxy.address}</span>
                    </div>
                  </TableCell>
                  <TableCell className="tnum px-1 text-xs md:px-3 md:text-sm">{proxy.port}</TableCell>
                  <TableCell className="max-w-0 px-1 md:px-3">
                    <span className="block truncate text-sm" title={`${reality.server_name}:${reality.server_port}`}>{reality.server_name}:{reality.server_port}</span>
                  </TableCell>
                  <TableCell className="px-0 text-center md:px-3">
                    <Switch checked={proxy.enabled} disabled={updatingProxyId !== null} onCheckedChange={(enabled) => setProxyEnabled(proxy, enabled)} aria-label={`${proxy.enabled ? "停用" : "启用"}代理 ${proxyName}`} />
                  </TableCell>
                  <TableCell className="whitespace-nowrap px-1 text-right md:px-3">
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
        description="删除会一并解除所有关联用户授权。此操作只修改配置数据，不会自动应用到服务器。"
        confirmLabel="删除代理"
        busy={removing}
        onClose={() => setDeleting(null)}
        onConfirm={remove}
      />}
      {trafficResetNode && <ConfirmDialog
        title={`清空节点「${trafficResetNode.node_name}」的业务流量？`}
        description="只清空 Hub 累计量并保留 Core counter baseline，下一次采集只会累计之后新增的流量。"
        confirmLabel="清空流量"
        busy={resettingTraffic}
        onClose={() => setTrafficResetNode(null)}
        onConfirm={() => void resetNodeTraffic()}
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

function UserForm({ user, go, onClose, onSaved }: {
  user: User | null
  go: Go
  onClose: () => void
  onSaved: () => void
}) {
  const [username, setUsername] = useState(user?.username ?? "")
  const [password, setPassword] = useState("")
  const [enabled, setEnabled] = useState(user?.enabled ?? true)
  const [forever, setForever] = useState(user?.expires_at === null || !user)
  const [expires, setExpires] = useState(localDate(user?.expires_at ?? null))
  const [saving, setSaving] = useState(false)
  const [resettingUuid, setResettingUuid] = useState(false)
  const [uuid, setUuid] = useState(user?.uuid ?? "")

  async function save(event: React.FormEvent) {
    event.preventDefault()
    if (!username.trim()) return toast.error("请填写用户名")
    if (!forever && !expires) return toast.error("请选择到期日期")
    if (!user && !password) return toast.error("请设置用户登录密码")
    const draft: UserDraft = {
      username: username.trim(),
      ...(password ? { password } : {}),
      enabled,
      expires_at: forever ? null : localExpiryBoundary(expires),
    }
    setSaving(true)
    try {
      if (user) await updateUser(user.id, draft)
      else await createUser(draft)
      toast.success(user
        ? "用户信息已更新，变更将在相关节点下次应用配置后生效。"
        : "用户已创建，变更将在相关节点下次应用配置后生效。")
      onClose()
      onSaved()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setSaving(false)
    }
  }

  async function resetUuid() {
    if (!user) return
    setResettingUuid(true)
    try {
      const result = await resetUserUuid(user.id)
      setUuid(result.uuid)
      onSaved()
      if (result.needs_sync_node_ids.length) {
        toast.warning(`UUID 已重置；节点 ${result.needs_sync_node_ids.join(", ")} 当前未能同步，需上线后应用配置。`)
      } else {
        toast.success(`UUID 已重置并已向 ${result.queued_node_ids.length} 个节点下发配置。`)
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "UUID 重置失败")
    } finally {
      setResettingUuid(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent onOpenAutoFocus={(e) => e.preventDefault()} className="flex max-h-[85vh] w-full max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-3xl">
        <form className="flex min-h-0 flex-1 flex-col" onSubmit={save}>
          <DialogHeader className="shrink-0 px-6 pt-6 pb-4">
            <DialogTitle>{user ? `编辑用户：${user.username}` : "新建用户"}</DialogTitle>
            <DialogDescription>{user ? "修改用户信息、有效期和账户状态。" : "创建代理用户，并设置账户状态和有效期。"}</DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 space-y-6 overflow-y-auto px-6 pb-5">
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

          <Field label="VLESS UUID" hint={user ? "UUID只用于代理认证；密码、资料和授权变更不会修改它。" : "系统创建用户时自动生成。"}>
            {user ? (
              <div className="flex flex-wrap items-center justify-between gap-3 rounded-md border bg-muted/30 px-3 py-2">
                <code className="min-w-0 flex-1 break-all font-mono text-sm select-all">{uuid}</code>
                <div className="flex shrink-0 items-center gap-2">
                  <Button type="button" variant="outline" disabled={!uuid} onClick={() => copyText(uuid)}><Copy />复制 UUID</Button>
                  <Button type="button" variant="outline" disabled={resettingUuid} onClick={() => void resetUuid()}><RefreshCw />重置 UUID</Button>
                </div>
              </div>
            ) : (
              <div className="rounded-md border bg-muted/30 px-3 py-2 text-sm text-muted-foreground">创建时自动生成</div>
            )}
          </Field>

          <Field label={user ? "到期时间（留空为永久有效）" : "有效期限"}>
            {!user && (
              <div className="grid gap-3 sm:grid-cols-2">
                <label className={`flex cursor-pointer items-center gap-3 rounded-lg border p-4 transition-colors ${forever ? "border-primary bg-primary/5" : "hover:bg-muted/50"}`}>
                  <input type="radio" name="expiry" className="accent-primary" checked={forever} onChange={() => setForever(true)} />
                  <span className="text-sm font-medium">永久有效</span>
                </label>
                <label className={`flex cursor-pointer items-center gap-3 rounded-lg border p-4 transition-colors ${!forever ? "border-primary bg-primary/5" : "hover:bg-muted/50"}`}>
                  <input type="radio" name="expiry" className="accent-primary" checked={!forever} onChange={() => setForever(false)} />
                  <span className="text-sm font-medium">指定到期时间</span>
                </label>
              </div>
            )}
            <Input
              className="mt-3"
              type="date"
              aria-label="到期时间"
              disabled={!user && forever}
              value={expires}
              onChange={(e) => {
                setExpires(e.target.value)
                if (user) setForever(!e.target.value)
              }}
            />
            {(!user ? !forever : Boolean(expires)) && <p className="text-xs text-muted-foreground">所选日期当天结束时到期，按当前浏览器时区计算。</p>}
          </Field>

          <div className="space-y-2">
            <Label className="text-sm font-medium">账户状态</Label>
            <label className="flex items-center justify-between rounded-lg border bg-muted/30 px-4 py-3">
              <span className="text-sm font-medium">启用用户</span>
              <Switch checked={enabled} onCheckedChange={setEnabled} aria-label="启用用户" />
            </label>
          </div>

          {user ? (
            <section className="space-y-2 border-t pt-5">
              <Label className="text-sm font-medium">代理授权</Label>
              <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-muted/30 px-4 py-3">
                <span className="text-sm">已授权 {user.proxy_count} 个代理</span>
                <Button type="button" variant="outline" size="sm" onClick={() => go(`/admin/subscriptions?user_id=${user.id}`)}>
                  管理授权 <KeyRound />
                </Button>
              </div>
            </section>
          ) : (
            <p className="rounded-lg border bg-muted/30 px-4 py-3 text-sm text-muted-foreground">创建用户后，可在「订阅」中为用户配置代理授权。</p>
          )}

          </div>
          <DialogFooter className="shrink-0 border-t px-6 py-4">
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving}>{user ? "保存修改" : "确认创建"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
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

function UsersPage({ go }: { go: Go }) {
  const [revision, setRevision] = useState(0)
  const [result, setResult] = useState<{ revision: number; items: User[] | null; error: string } | null>(null)
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
    listUsers().then((users) => {
      if (active) setResult({ revision, items: users, error: "" })
    }).catch((e: Error) => {
      if (active) setResult({ revision, items: null, error: e.message })
    })
    return () => { active = false }
  }, [revision])

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

  const current = result?.revision === revision
  const items = current ? result.items : null
  const error = current ? result.error : ""
  const loading = !current
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
      await deleteUser(deleting.id)
      toast.success("用户及其授权已删除，变更将在相关节点下次应用配置后生效。")
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
      await updateUser(user.id, { username: user.username, enabled, expires_at: user.expires_at })
      toast.success("用户状态已更新，相关代理配置将在对应节点下次应用配置后生效。")
      reload()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setUpdatingUserId(null)
    }
  }

  function reload() {
    setRevision((value) => value + 1)
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
            <TableHead className="w-[8%] px-4">ID</TableHead>
            <TableHead className="w-[22%] px-4">用户名</TableHead>
            <TableHead className="w-[18%] px-4">代理授权</TableHead>
            <TableHead className="w-[20%] px-4">到期时间</TableHead>
            <TableHead className="w-[17%] px-4">账户状态</TableHead>
            <TableHead className="w-[20%] px-4">业务流量</TableHead>
            <TableHead className="text-right">操作</TableHead>
          </TableRow></TableHeader>
          <TableBody>
            {visible.map((user) => (
              <TableRow key={user.id} className="h-14">
                <TableCell className="tnum px-4 text-sm text-muted-foreground">{user.id}</TableCell>
                <TableCell className="px-4"><div className="font-semibold">{user.username}</div></TableCell>
                <TableCell className="px-4 text-sm">{user.proxy_count} 个代理</TableCell>
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
                <TableCell className="px-4">
                  <div className="space-y-1 text-xs text-muted-foreground">
                    <div>上行 {bytes(trafficByUser.get(user.id)?.uplink_bytes ?? 0)}</div>
                    <div>下行 {bytes(trafficByUser.get(user.id)?.downlink_bytes ?? 0)}</div>
                  </div>
                  <Button variant="link" size="sm" className="h-7 px-0" onClick={() => setTrafficUser(user)}>明细 / 清零</Button>
                </TableCell>
                <TableCell className="whitespace-nowrap text-right">
                  <div className="flex items-center justify-end gap-1">
                    <Button variant="ghost" size="sm" onClick={() => setEditing(user)}><Pencil /> 编辑</Button>
                    <Button variant="ghost" size="sm" className="text-primary hover:text-primary" onClick={() => go(`/admin/subscriptions?user_id=${user.id}`)}><KeyRound /> 管理授权</Button>
                    <Button variant="ghost" size="sm" disabled={user.username === "admin"} title={user.username === "admin" ? "默认用户 admin 不可删除" : undefined} className="text-destructive hover:text-destructive" onClick={() => setDeleting(user)}><Trash2 /> 删除</Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}
            {!loading && !error && !visible.length && <TableRow><TableCell colSpan={7} className="py-10 text-center text-sm text-muted-foreground">{items?.length ? "没有匹配的用户" : "还没有用户，右上角新增"}</TableCell></TableRow>}
            {loading && <TableRow><TableCell colSpan={7} className="p-4"><Skeleton className="h-10 w-full" /></TableCell></TableRow>}
            {error && <TableRow><TableCell colSpan={7} className="py-8 text-center text-sm text-destructive">
              <div role="alert">加载用户失败：{error}</div>
              <Button variant="outline" size="sm" className="mt-3" onClick={reload}>重试</Button>
            </TableCell></TableRow>}
          </TableBody>
        </Table>
      </Card>
      {creating && <UserForm user={null} go={go} onClose={() => setCreating(false)} onSaved={reload} />}
      {editing && <UserForm user={editing} go={go} onClose={() => setEditing(null)} onSaved={reload} />}
      {trafficUser && <UserTrafficDialog user={trafficUser} onClose={() => setTrafficUser(null)} onChanged={() => setTrafficRevision((value) => value + 1)} />}
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

function SubscriptionUserPicker({ users, value, onChange }: { users: User[]; value: number | null; onChange: (userId: number) => void }) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState("")
  const selected = users.find((user) => user.id === value)
  const visible = users.filter((user) => user.username.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
  return (
    <Popover open={open} onOpenChange={(next) => { setOpen(next); if (!next) setQuery("") }}>
      <PopoverTrigger asChild>
        <Button type="button" variant="outline" className="w-full justify-between font-normal" aria-label="搜索并选择用户">
          <span className="truncate">{selected?.username ?? "搜索 / 选择用户"}</span><ChevronDown className="size-4 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-[var(--radix-popover-trigger-width)] p-2">
        <Input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索用户名" aria-label="搜索用户名" />
        <div className="mt-2 max-h-56 overflow-y-auto">
          {visible.map((user) => (
            <button key={user.id} type="button" className={`flex w-full items-center justify-between rounded-sm px-2 py-2 text-left text-sm hover:bg-accent ${user.id === value ? "bg-accent" : ""}`} onClick={() => { onChange(user.id); setOpen(false); setQuery("") }}>
              <span>{user.username}</span>
            </button>
          ))}
          {!visible.length && <p className="px-2 py-3 text-sm text-muted-foreground">没有匹配的用户</p>}
        </div>
      </PopoverContent>
    </Popover>
  )
}

function SubscriptionForm({ mode, users, user, proxies, nodes, accesses, onClose, onSaved }: {
  mode: "create" | "edit"
  users: User[]
  user: User | null
  proxies: Proxy[]
  nodes: Node[]
  accesses: UserProxyAuthorization[]
  onClose: () => void
  onSaved: () => void
}) {
  const creating = mode === "create"
  const eligibleUsers = users.filter((item) => item.proxy_count === 0)
  const [userId, setUserId] = useState<number | null>(user?.id ?? null)
  const [proxyQuery, setProxyQuery] = useState("")
  const [expandedGroups, setExpandedGroups] = useState<Record<number, boolean>>({})
  const [selectedIds, setSelectedIds] = useState<Set<number>>(() => new Set(accesses.map((item) => item.proxy.id)))
  const [authSettings, setAuthSettings] = useState(() => existingAuthorizationSettings(accesses))
  const [authOpen, setAuthOpen] = useState(false)
  const [authProxyId, setAuthProxyId] = useState<number | null>(() => accesses[0]?.proxy.id ?? null)
  const [saving, setSaving] = useState(false)
  const userOptions = creating ? eligibleUsers : user ? [user] : []
  const selectedUser = userOptions.find((item) => item.id === userId) ?? null
  const groups = useMemo(() => groupProxiesByNode(proxies, nodes), [proxies, nodes])
  const nodeById = useMemo(() => new Map(nodes.map((node) => [node.id, node])), [nodes])
  const selectedProxies = proxies.filter((proxy) => selectedIds.has(proxy.id))
  const currentAuthProxy = selectedProxies.find((proxy) => proxy.id === authProxyId) ?? selectedProxies[0] ?? null
  const currentAuth = currentAuthProxy ? authSettings[currentAuthProxy.id] : undefined
  const query = proxyQuery.trim().toLocaleLowerCase()
  const existingById = new Map(accesses.map((item) => [item.proxy.id, item]))

  function setProxySelected(proxyId: number, checked: boolean) {
    setSelectedIds((current) => {
      const next = new Set(current)
      if (checked) next.add(proxyId)
      else next.delete(proxyId)
      return next
    })
    if (checked) {
      setAuthSettings((current) => current[proxyId] ? current : { ...current, [proxyId]: { flow: "" as Flow, enabled: true } })
      setAuthProxyId((current) => current ?? proxyId)
    }
  }

  function selectFormUser(nextUserId: number) {
    if (nextUserId !== userId) {
      setSelectedIds(new Set())
      setAuthSettings({})
      setAuthProxyId(null)
    }
    setUserId(nextUserId)
  }

  function setGroupSelected(proxyIds: number[], checked: boolean) {
    const nextIds = toggleProxyGroup(selectedIds, proxyIds)
    if (!checked) {
      const selectedAfterToggle = new Set(nextIds)
      setSelectedIds(selectedAfterToggle)
      return
    }
    setSelectedIds(new Set(nextIds))
    const missing = proxyIds.filter((id) => !authSettings[id])
    if (missing.length) {
      setAuthSettings((current) => {
        const next = { ...current }
        for (const id of missing) next[id] = { flow: "", enabled: true }
        return next
      })
    }
    setAuthProxyId((current) => current ?? proxyIds[0] ?? null)
  }

  function patchAuth(proxyId: number, values: Partial<{ flow: Flow; enabled: boolean }>) {
    setAuthSettings((current) => ({ ...current, [proxyId]: { ...current[proxyId], ...values } }))
  }

  async function save(event: React.FormEvent) {
    event.preventDefault()
    if (!selectedUser) return toast.error("请选择用户")
    if (!selectedIds.size) return toast.error("请至少选择一个授权节点")
    const operations: Array<Promise<unknown>> = []
    for (const proxy of selectedProxies) {
      const existing = existingById.get(proxy.id)
      const nextAuth = authSettings[proxy.id]
      if (!existing || existing.access.auth.flow !== nextAuth.flow || existing.access.enabled !== nextAuth.enabled) {
        operations.push(saveUserProxy(selectedUser.id, proxy.id, {
          enabled: nextAuth.enabled,
          auth: { flow: nextAuth.flow },
        }))
      }
    }
    if (!creating) {
      for (const existing of accesses) {
        if (!selectedIds.has(existing.proxy.id)) operations.push(deleteUserProxy(selectedUser.id, existing.proxy.id))
      }
    }

    setSaving(true)
    try {
      const results = await Promise.allSettled(operations)
      const failed = results.filter((result) => result.status === "rejected")
      if (failed.length) {
        toast.error(`有 ${failed.length} 项授权未能保存，已刷新当前状态。`)
        onClose()
        onSaved()
        return
      }
      toast.success(creating ? "订阅已创建，授权将在相关节点下次应用配置后生效。" : "订阅已更新，授权将在相关节点下次应用配置后生效。")
      onClose()
      onSaved()
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent onOpenAutoFocus={(event) => event.preventDefault()} className="flex max-h-[80vh] w-full max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-3xl">
        <form className="flex min-h-0 flex-1 flex-col" onSubmit={save}>
          <DialogHeader className="shrink-0 px-6 pt-6 pb-4">
            <DialogTitle>{creating ? "新增订阅" : `编辑订阅：${user?.username ?? ""}`}</DialogTitle>
            <DialogDescription>{creating ? "为用户选择可使用的代理节点。" : "调整该用户的代理节点授权。"}</DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 space-y-4 overflow-hidden px-6 pb-5">
            <Field label="选择用户 *">
              {creating ? (
                <SubscriptionUserPicker users={eligibleUsers} value={userId} onChange={selectFormUser} />
              ) : (
                <Input value={user?.username ?? ""} disabled readOnly />
              )}
              {creating && !eligibleUsers.length && <p className="text-xs text-muted-foreground">所有用户都已有订阅授权，或尚未创建用户。</p>}
            </Field>

            <Field label="选择授权节点 *">
              <AdminSearchInput value={proxyQuery} onChange={setProxyQuery} placeholder="搜索节点名称或所属服务器..." className="w-full" />
              <div className="max-h-[34vh] min-h-28 overflow-y-auto rounded-md border">
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
                          onChange={(event) => setGroupSelected(proxyIds, event.target.checked)}
                          aria-label={`选择服务器 ${group.node?.name ?? `节点 ${group.nodeId}`}`}
                        />
                        <button type="button" className="flex min-w-0 flex-1 items-center justify-between gap-3 text-left text-sm" onClick={() => setExpandedGroups((current) => ({ ...current, [group.nodeId]: !isExpanded }))}>
                          <span className="truncate font-medium">{group.node?.name ?? `节点 ${group.nodeId}`}</span>
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
                                <span className="truncate text-sm font-medium">{displayProxyName(proxy, node)}</span>
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
            </Field>

            <div className="flex items-center justify-between border-t pt-3">
              <span className="text-sm text-muted-foreground">已选择 {selectedIds.size} 个节点</span>
              <Button type="button" variant="ghost" size="sm" disabled={!selectedIds.size} onClick={() => setAuthOpen((open) => !open)} aria-expanded={authOpen}>
                {authOpen ? "收起认证配置" : "认证配置"}{authOpen ? <ChevronUp /> : <ChevronDown />}
              </Button>
            </div>
            {authOpen && selectedProxies.length > 0 && (
              <div className="space-y-3 rounded-md border bg-muted/20 p-3">
                <Field label="选择授权节点">
                  <Select value={String(currentAuthProxy?.id ?? "")} onValueChange={(value) => setAuthProxyId(Number(value))}>
                    <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                    <SelectContent position="popper">
                      {selectedProxies.map((proxy) => <SelectItem key={proxy.id} value={String(proxy.id)}>{displayProxyName(proxy, nodeById.get(proxy.node_id))}</SelectItem>)}
                    </SelectContent>
                  </Select>
                </Field>
                {currentAuthProxy && currentAuth && <div className="grid gap-3 sm:grid-cols-2">
                  <label className="flex items-center justify-between rounded-md border bg-background px-3 py-2 sm:col-span-2">
                    <span className="text-sm font-medium">启用该节点授权</span>
                    <Switch checked={currentAuth.enabled} onCheckedChange={(enabled) => patchAuth(currentAuthProxy.id, { enabled })} aria-label={`启用${displayProxyName(currentAuthProxy, nodeById.get(currentAuthProxy.node_id))}授权`} />
                  </label>
                  <Field label="Flow">
                    <Select value={currentAuth.flow || "none"} onValueChange={(value) => patchAuth(currentAuthProxy.id, { flow: value === "none" ? "" : value as Flow })}>
                      <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                      <SelectContent position="popper"><SelectItem value="none">无</SelectItem><SelectItem value="xtls-rprx-vision">xtls-rprx-vision</SelectItem></SelectContent>
                    </Select>
                  </Field>
                </div>}
              </div>
            )}
          </div>
          <DialogFooter className="shrink-0 border-t px-6 py-4">
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving || !selectedUser || !selectedIds.size || !proxies.length}>{saving ? "保存中..." : creating ? "创建订阅" : "保存修改"}</Button>
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
    <Table className="table-fixed">
      <TableHeader className="bg-muted/30"><TableRow>
        <TableHead className="w-[17%] max-md:w-[20%] px-1 md:px-3">节点</TableHead>
        <TableHead className="w-[14%] max-md:w-[22%] px-1 md:px-3">名称</TableHead>
        <TableHead className="hidden w-[16%] px-3 md:table-cell">协议</TableHead>
        <TableHead className="hidden w-[20%] px-3 md:table-cell">地址</TableHead>
        <TableHead className="w-[8%] max-md:w-[12%] px-1 md:px-3">端口</TableHead>
        <TableHead className="w-[25%] max-md:w-[46%] px-1 md:px-3">SNI</TableHead>
      </TableRow></TableHeader>
      <TableBody>
        {ordered.map((access) => {
          const proxy = proxyById.get(access.proxy.id)
          const node = nodeById.get(access.proxy.node_id)
          const reality = proxy?.config.reality
          return <TableRow key={access.proxy.id} className="h-12">
            <TableCell className="max-w-0 px-1 md:px-3"><span className="block truncate font-medium" title={node?.name}>{node?.name ?? `节点 ${access.proxy.node_id}`}</span></TableCell>
            <TableCell className="max-w-0 px-1 md:px-3"><span className="block truncate font-medium" title={displayProxyName(access.proxy, node)}>{displayProxyName(access.proxy, node)}</span></TableCell>
            <TableCell className="hidden px-3 md:table-cell"><ProxyProtocolBadge protocol={access.proxy.protocol} /></TableCell>
            <TableCell className="hidden max-w-0 px-3 md:table-cell"><div className="flex min-w-0 items-center gap-2"><ProxyAddressTypeBadge addressType={access.proxy.address_type} /><span className="truncate text-sm" title={access.proxy.address}>{access.proxy.address}</span></div></TableCell>
            <TableCell className="tnum px-1 text-xs md:px-3 md:text-sm">{access.proxy.port}</TableCell>
            <TableCell className="max-w-0 px-1 md:px-3"><span className="block truncate text-sm" title={reality ? `${reality.server_name}:${reality.server_port}` : ""}>{reality ? `${reality.server_name}:${reality.server_port}` : "—"}</span></TableCell>
          </TableRow>
        })}
        {!ordered.length && <TableRow><TableCell colSpan={6} className="py-5 text-center text-sm text-muted-foreground">该用户还没有代理节点授权</TableCell></TableRow>}
      </TableBody>
    </Table>
  )
}

function SubscriptionsPage({ nodes, search }: { nodes: Node[]; search: string }) {
  const { items: proxies, error: proxyError, loading: proxiesLoading, reload: reloadProxies } = useAllProxies(nodes)
  const requestedId = new URLSearchParams(search).get("user_id")
  const requestedUserId = requestedId && /^\d+$/.test(requestedId) ? Number(requestedId) : null
  const [usersRevision, setUsersRevision] = useState(0)
  const [usersResult, setUsersResult] = useState<{ revision: number; items: User[] | null; error: string } | null>(null)
  const [userQuery, setUserQuery] = useState("")
  const [status, setStatus] = useState("all")
  const [accessCache, setAccessCache] = useState<Map<number, UserProxyAuthorization[]>>(() => new Map())
  const accessCacheRef = useRef(accessCache)
  const accessRequests = useRef(new Map<number, Promise<UserProxyAuthorization[]>>())
  const [accessLoading, setAccessLoading] = useState<Set<number>>(() => new Set())
  const [accessErrors, setAccessErrors] = useState<Record<number, string>>({})
  const [expandedUsers, setExpandedUsers] = useState<Set<number>>(() => requestedUserId === null ? new Set() : new Set([requestedUserId]))
  const [creating, setCreating] = useState(false)
  const [editing, setEditing] = useState<User | null>(null)
  const [deleting, setDeleting] = useState<User | null>(null)
  const [removing, setRemoving] = useState(false)
  const [updatingUserId, setUpdatingUserId] = useState<number | null>(null)
  const currentUsers = usersResult?.revision === usersRevision
  const users = currentUsers ? usersResult?.items ?? null : null
  const usersLoading = !currentUsers
  const usersError = currentUsers ? usersResult?.error ?? "" : ""
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
  const eligibleUsers = (users ?? []).filter((user) => user.proxy_count === 0)

  const loadAccesses = useCallback((userId: number): Promise<UserProxyAuthorization[]> => {
    const cached = accessCacheRef.current.get(userId)
    if (cached) return Promise.resolve(cached)
    const pending = accessRequests.current.get(userId)
    if (pending) return pending
    setAccessLoading((current) => new Set(current).add(userId))
    setAccessErrors((current) => { const next = { ...current }; delete next[userId]; return next })
    const request = listUserAuthorizations(userId).then((items) => {
      const next = new Map(accessCacheRef.current)
      next.set(userId, items)
      accessCacheRef.current = next
      setAccessCache(next)
      return items
    }).catch((error: Error) => {
      setAccessErrors((current) => ({ ...current, [userId]: error.message }))
      throw error
    }).finally(() => {
      accessRequests.current.delete(userId)
      setAccessLoading((current) => { const next = new Set(current); next.delete(userId); return next })
    })
    accessRequests.current.set(userId, request)
    return request
  }, [])

  useEffect(() => {
    let active = true
    listUsers().then((items) => {
      if (active) setUsersResult({ revision: usersRevision, items, error: "" })
    }).catch((error: Error) => {
      if (active) setUsersResult({ revision: usersRevision, items: null, error: error.message })
    })
    return () => { active = false }
  }, [usersRevision])

  useEffect(() => {
    if (requestedUserId !== null && users?.some((user) => user.id === requestedUserId)) {
      void loadAccesses(requestedUserId).catch(() => {})
    }
  }, [requestedUserId, users, loadAccesses])

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
  }, [needle, userKey, loadAccesses])

  async function toggleExpanded(user: User) {
    const isOpen = expandedUsers.has(user.id)
    setExpandedUsers((current) => { const next = new Set(current); if (isOpen) next.delete(user.id); else next.add(user.id); return next })
    if (!isOpen) void loadAccesses(user.id).catch(() => {})
  }

  async function openEdit(user: User) {
    try {
      await loadAccesses(user.id)
      setEditing(user)
    } catch (error) {
      toast.error(`加载 ${user.username} 的授权失败：${(error as Error).message}`)
    }
  }

  async function openDelete(user: User) {
    try {
      await loadAccesses(user.id)
      setDeleting(user)
    } catch (error) {
      toast.error(`加载 ${user.username} 的授权失败：${(error as Error).message}`)
    }
  }

  async function setUserEnabled(user: User, enabled: boolean) {
    setUpdatingUserId(user.id)
    try {
      await updateUser(user.id, { username: user.username, enabled, expires_at: user.expires_at })
      toast.success(enabled ? "订阅授权已启用，变更将在相关节点下次应用配置后生效。" : "订阅授权已停用，用户与节点关联关系已保留。")
      setUsersRevision((value) => value + 1)
    } catch (error) {
      toast.error((error as Error).message)
    } finally {
      setUpdatingUserId(null)
    }
  }

  async function removeSubscription() {
    if (!deleting) return
    setRemoving(true)
    const grants = accessCache.get(deleting.id) ?? []
    try {
      const results = await Promise.allSettled(grants.map((item) => deleteUserProxy(deleting.id, item.proxy.id)))
      const failed = results.filter((result) => result.status === "rejected")
      if (failed.length) toast.error(`有 ${failed.length} 条授权未能删除，请刷新后重试。`)
      else toast.success(`已删除 ${deleting.username} 的订阅授权关系；用户和代理节点未删除。`)
      setDeleting(null)
      reloadAll()
    } finally {
      setRemoving(false)
    }
  }

  function reloadAll() {
    accessCacheRef.current = new Map()
    setAccessCache(new Map())
    setAccessErrors({})
    setUsersRevision((value) => value + 1)
    reloadProxies()
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
          <Button disabled={usersLoading || !eligibleUsers.length || proxiesLoading || !proxies?.length || !!usersError || !!proxyError} onClick={() => setCreating(true)}><Plus /> 新增订阅</Button>
        </div>

        <Card className="overflow-x-auto p-0">
          <Table>
            <TableHeader className="bg-muted/50"><TableRow>
              <TableHead className="w-[38%] px-3">用户</TableHead>
              <TableHead className="w-[34%] px-3">授权节点</TableHead>
              <TableHead className="w-[12%] px-3 text-center">状态</TableHead>
              <TableHead className="w-[16%] px-3 text-right">操作</TableHead>
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
                    <TableCell className="px-3 text-sm">{user.proxy_count} 个节点</TableCell>
                    <TableCell className="px-3 text-center"><Switch checked={user.enabled} disabled={updatingUserId !== null} onCheckedChange={(enabled) => void setUserEnabled(user, enabled)} aria-label={`${user.enabled ? "停用" : "启用"}${user.username}的订阅授权`} /></TableCell>
                    <TableCell className="whitespace-nowrap px-3 text-right">
                      <div className="flex items-center justify-end gap-1">
                        <Tooltip><TooltipTrigger asChild><Button type="button" variant="ghost" size="icon" aria-label="编辑订阅" onClick={() => void openEdit(user)}><Pencil /></Button></TooltipTrigger><TooltipContent>编辑订阅</TooltipContent></Tooltip>
                        <Tooltip><TooltipTrigger asChild><Button type="button" variant="ghost" size="icon" disabled={!user.proxy_count} className="text-destructive hover:text-destructive" aria-label="删除订阅授权" onClick={() => void openDelete(user)}><Trash2 /></Button></TooltipTrigger><TooltipContent>{user.proxy_count ? "删除订阅授权" : "当前没有授权节点"}</TooltipContent></Tooltip>
                      </div>
                    </TableCell>
                  </TableRow>
                  {isExpanded && <TableRow className="bg-muted/20 hover:bg-muted/20"><TableCell colSpan={4} className="p-0">
                    {rowError ? <div className="p-4 text-sm text-destructive" role="alert">加载授权节点失败：{rowError}<Button variant="link" className="h-auto p-0 pl-2" onClick={() => void loadAccesses(user.id).catch(() => {})}>重试</Button></div>
                      : accessLoading.has(user.id) || proxiesLoading ? <div className="p-4"><Skeleton className="h-9 w-full" /></div>
                      : accesses ? <SubscriptionProxyRows accesses={accesses} proxies={proxies ?? []} nodes={nodes} /> : null}
                  </TableCell></TableRow>}
                </Fragment>
              })}
              {!usersLoading && !usersError && !visibleUsers.length && <TableRow><TableCell colSpan={4} className="py-10 text-center text-sm text-muted-foreground">{matchingAccessesPending ? "正在搜索授权节点..." : users?.length ? "没有匹配的用户或节点" : "还没有用户"}</TableCell></TableRow>}
              {usersLoading && <TableRow><TableCell colSpan={4} className="p-4"><Skeleton className="h-10 w-full" /></TableCell></TableRow>}
              {usersError && <TableRow><TableCell colSpan={4} className="py-8 text-center text-sm text-destructive"><div role="alert">加载用户失败：{usersError}</div><Button variant="outline" size="sm" className="mt-3" onClick={() => setUsersRevision((value) => value + 1)}>重试</Button></TableCell></TableRow>}
              {proxyError && <TableRow><TableCell colSpan={4} className="py-8 text-center text-sm text-destructive"><div role="alert">加载代理失败：{proxyError}</div><Button variant="outline" size="sm" className="mt-3" onClick={reloadProxies}>重试</Button></TableCell></TableRow>}
            </TableBody>
          </Table>
        </Card>

        {creating && <SubscriptionForm key="create" mode="create" users={users ?? []} user={null} proxies={proxies ?? []} nodes={nodes} accesses={[]} onClose={() => setCreating(false)} onSaved={reloadAll} />}
        {editing && <SubscriptionForm key={`edit-${editing.id}`} mode="edit" users={users ?? []} user={editing} proxies={proxies ?? []} nodes={nodes} accesses={accessCache.get(editing.id) ?? []} onClose={() => setEditing(null)} onSaved={reloadAll} />}
        {deleting && <ConfirmDialog
          title={`确认删除 ${deleting.username} 的订阅？`}
          description={`删除后该用户将失去当前全部 ${deleting.proxy_count} 个代理节点授权，但不会删除用户、代理节点或服务器。`}
          confirmLabel="删除"
          busy={removing}
          onClose={() => setDeleting(null)}
          onConfirm={() => void removeSubscription()}
        />}
      </div>
    </TooltipProvider>
  )
}

export function ProxiesPage({ nodes }: { nodes: Node[] }) {
  return <ProxyPage nodes={nodes} />
}

export function UsersResourcePage({ go }: { go: Go }) {
  return <UsersPage go={go} />
}

export function SubscriptionsResourcePage({ nodes, search }: { nodes: Node[]; search: string; go: Go }) {
  return <SubscriptionsPage nodes={nodes} search={search} />
}
