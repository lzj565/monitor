import { useCallback, useEffect, useState } from "react"
import { LoaderCircle, Plus, RotateCw } from "lucide-react"
import { toast } from "sonner"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Skeleton } from "@/components/ui/skeleton"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { api, type Node, type ProxyNode } from "@/lib/api"

type AddressMode = "ipv4" | "ipv6" | "custom"
type SubmitStage = "creating" | "deploying"
type SingboxStatus = { installed?: boolean; service_exists?: boolean; running?: boolean; service_state_known?: boolean }
type NewProxyNodeForm = {
  nodeId: string
  name: string
  addressMode: AddressMode
  customAddress: string
  realityServerName: string
  realityDest: string
}

const EMPTY_FORM: NewProxyNodeForm = {
  nodeId: "",
  name: "",
  addressMode: "ipv4",
  customAddress: "",
  realityServerName: "www.apple.com",
  realityDest: "www.apple.com:443",
}

function connectionAddress(addressMode: string, customAddress: string | null | undefined, server: Node | undefined) {
  if (addressMode === "custom") return customAddress?.trim() || ""
  if (addressMode === "ipv4") return server?.ipv4_pin || server?.ipv4_auto || server?.ipv4 || ""
  if (addressMode === "ipv6") return server?.ipv6_pin || server?.ipv6_auto || server?.ipv6 || ""
  return ""
}

function protocolLabel(protocol: string) {
  return protocol === "vless_reality" ? "VLESS + Reality" : "未知协议"
}

function addressForServer(server: Node) {
  return server.ipv4_pin || server.ipv4_auto || server.ipv4 || server.ipv6_pin || server.ipv6_auto || server.ipv6 || ""
}

function serverReadiness(server: Node, status: SingboxStatus | undefined, busy: string | undefined) {
  if (!server.online) return { ready: false, label: "Agent 离线" }
  if (busy === "status") return { ready: false, label: "正在查询 sing-box" }
  if (busy) return { ready: false, label: "sing-box 操作中" }
  if (status?.installed === false) return { ready: false, label: "sing-box 未安装" }
  if (!status) return { ready: false, label: "sing-box 状态未知" }
  if (status.service_state_known === false) return { ready: false, label: "无法确认 sing-box 状态" }
  if (status.installed !== true) return { ready: false, label: "sing-box 安装状态未知" }
  if (status.service_exists !== true || status.running !== true) return { ready: false, label: "sing-box 未运行" }
  return { ready: true, label: "sing-box 运行中" }
}

function loadProxyNodes() {
  return api<{ nodes: ProxyNode[] }>("/proxy/nodes", { cache: "no-store" })
}

function DeploymentBadge({ proxyNode }: { proxyNode: ProxyNode }) {
  switch (proxyNode.deploy_status) {
    case "deployed":
      return <Badge className="border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900 dark:bg-emerald-950 dark:text-emerald-300">已部署</Badge>
    case "deploying":
      return <Badge variant="secondary"><LoaderCircle className="animate-spin" />部署中</Badge>
    case "failed":
      return <Badge variant="destructive" title={proxyNode.last_error || "部署失败，暂无错误详情"}>部署失败</Badge>
    case "not_deployed":
      return <Badge variant="secondary">未部署</Badge>
    default:
      return <Badge variant="outline">状态未知</Badge>
  }
}

export function ProxyNodeManager({
  nodes,
  liveStatuses,
  rowBusy,
}: {
  nodes: Node[]
  liveStatuses: Record<number, SingboxStatus>
  rowBusy: Record<number, string>
}) {
  const [proxyNodes, setProxyNodes] = useState<ProxyNode[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState("")
  const [deploying, setDeploying] = useState<Record<number, boolean>>({})
  const [createOpen, setCreateOpen] = useState(false)
  const [form, setForm] = useState<NewProxyNodeForm>(EMPTY_FORM)
  const [formError, setFormError] = useState("")
  const [submitStage, setSubmitStage] = useState<SubmitStage | null>(null)
  const serversById = new Map(nodes.map((server) => [server.id, server]))
  const selectedServer = nodes.find((server) => server.id.toString() === form.nodeId)
  const selectedReadiness = selectedServer
    ? serverReadiness(selectedServer, liveStatuses[selectedServer.id], rowBusy[selectedServer.id])
    : null

  const refresh = useCallback(async () => {
    try {
      const result = await loadProxyNodes()
      setError("")
      setProxyNodes(result.nodes)
      return result.nodes
    } catch (cause) {
      setError((cause as Error).message)
      return null
    } finally {
      setLoading(false)
    }
  }, [])

  function openCreate() {
    setForm({ ...EMPTY_FORM })
    setFormError("")
    setCreateOpen(true)
  }

  useEffect(() => {
    let active = true
    void loadProxyNodes().then((result) => {
      if (active) {
        setError("")
        setProxyNodes(result.nodes)
      }
    }).catch((cause) => {
      if (active) setError((cause as Error).message)
    }).finally(() => {
      if (active) setLoading(false)
    })
    return () => { active = false }
  }, [])

  async function redeploy(nodeId: number) {
    if (deploying[nodeId]) return
    setDeploying((current) => ({ ...current, [nodeId]: true }))
    let deployed = false
    try {
      await api(`/proxy/servers/${nodeId}/deploy`, { method: "POST" })
      deployed = true
      toast.success("代理节点已重新部署")
    } catch (cause) {
      toast.error(`代理节点重新部署失败：${(cause as Error).message}`)
    } finally {
      const refreshed = await refresh()
      if (!refreshed && deployed) toast.error("部署成功，但代理节点列表刷新失败")
      setDeploying((current) => {
        const next = { ...current }
        delete next[nodeId]
        return next
      })
    }
  }

  async function createAndDeploy(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (submitStage) return
    setFormError("")
    if (!selectedServer || !selectedReadiness?.ready) {
      setFormError("请选择 Agent 在线且 sing-box 正在运行的服务器")
      return
    }
    if (!form.name.trim()) {
      setFormError("请填写代理节点名称")
      return
    }
    if (form.addressMode === "custom" && !form.customAddress.trim()) {
      setFormError("请填写自定义连接地址")
      return
    }

    setSubmitStage("creating")
    let created: { node: ProxyNode }
    try {
      created = await api<{ node: ProxyNode }>("/proxy/nodes", {
        method: "POST",
        body: JSON.stringify({
          node_id: selectedServer.id,
          name: form.name.trim(),
          address_mode: form.addressMode,
          custom_address: form.addressMode === "custom" ? form.customAddress.trim() : null,
          listen_port: null,
          reality_server_name: form.realityServerName.trim(),
          reality_dest: form.realityDest.trim(),
        }),
      })
    } catch (cause) {
      setFormError((cause as Error).message)
      setSubmitStage(null)
      return
    }

    setSubmitStage("deploying")
    let deployed = false
    try {
      await api(`/proxy/servers/${created.node.node_id}/deploy`, { method: "POST" })
      deployed = true
    } catch {
      // 部署接口会把可确认的失败写入所有相关代理节点的状态。
    }
    const refreshed = await refresh()
    setCreateOpen(false)
    setSubmitStage(null)
    if (deployed) toast.success("代理节点创建并部署成功")
    else toast.error("代理节点已创建，但部署失败")
    if (!refreshed) toast.error("代理节点列表刷新失败")
  }

  return (
    <Card className="gap-4 p-5">
      <div className="space-y-1">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="text-base font-semibold">已配置的代理节点</h2>
          <Button onClick={openCreate}><Plus />新建代理节点</Button>
        </div>
        <p className="text-sm text-muted-foreground">管理服务器上的 sing-box 代理节点</p>
      </div>
      {error && (
        <div className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-destructive/30 p-3 text-sm">
          <p role="alert" className="text-destructive">加载代理节点失败：{error}</p>
          <Button variant="outline" size="sm" onClick={() => { setError(""); setLoading(true); void refresh() }}>重试</Button>
        </div>
      )}
      {loading ? (
        <div className="space-y-3" aria-label="正在加载代理节点">
          <Skeleton className="h-10" />
          <Skeleton className="h-12" />
          <Skeleton className="h-12" />
        </div>
      ) : !error && proxyNodes.length === 0 ? (
        <div className="rounded-md border border-dashed px-4 py-12 text-center">
          <p className="font-medium">暂无代理节点</p>
          <p className="mt-1 text-sm text-muted-foreground">创建第一个 VLESS Reality 代理节点。</p>
          <Button className="mt-4" onClick={openCreate}><Plus />新建代理节点</Button>
        </div>
      ) : proxyNodes.length > 0 ? (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>所属服务器</TableHead>
              <TableHead>代理节点名称</TableHead>
              <TableHead>协议</TableHead>
              <TableHead>连接地址</TableHead>
              <TableHead>端口</TableHead>
              <TableHead>SNI</TableHead>
              <TableHead>部署状态</TableHead>
              <TableHead>操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {proxyNodes.map((proxyNode) => {
              const server = serversById.get(proxyNode.node_id)
              const serverBusy = Boolean(deploying[proxyNode.node_id])
              return (
                <TableRow key={proxyNode.id}>
                  <TableCell className="font-medium">{server?.name || `服务器 #${proxyNode.node_id}`}</TableCell>
                  <TableCell>{proxyNode.name}</TableCell>
                  <TableCell><Badge variant="secondary">{protocolLabel(proxyNode.protocol)}</Badge></TableCell>
                  <TableCell className="max-w-52 truncate" title={connectionAddress(proxyNode.address_mode, proxyNode.custom_address, server) || undefined}>
                    {connectionAddress(proxyNode.address_mode, proxyNode.custom_address, server) || "—"}
                  </TableCell>
                  <TableCell className="tnum">{proxyNode.listen_port}</TableCell>
                  <TableCell>{proxyNode.reality_server_name || "—"}</TableCell>
                  <TableCell><DeploymentBadge proxyNode={proxyNode} /></TableCell>
                  <TableCell>
                    {proxyNode.deploy_status === "failed" && (
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={serverBusy}
                        onClick={() => void redeploy(proxyNode.node_id)}
                      >
                        <RotateCw className={serverBusy ? "animate-spin" : undefined} />
                        {serverBusy ? "部署中…" : "重新部署"}
                      </Button>
                    )}
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      ) : null}
      <Dialog open={createOpen} onOpenChange={(open) => { if (!submitStage) setCreateOpen(open) }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>新建代理节点</DialogTitle>
            <DialogDescription>在 sing-box 正在运行的服务器上创建 VLESS + Reality 代理入口。</DialogDescription>
          </DialogHeader>
          <form className="space-y-4" onSubmit={(event) => void createAndDeploy(event)}>
            <div className="space-y-2">
              <Label htmlFor="proxy-server">所属服务器 *</Label>
              <Select value={form.nodeId} disabled={Boolean(submitStage)} onValueChange={(nodeId) => setForm((current) => ({ ...current, nodeId }))}>
                <SelectTrigger id="proxy-server" className="w-full"><SelectValue placeholder="选择服务器" /></SelectTrigger>
                <SelectContent>
                  {nodes.map((server) => {
                    const readiness = serverReadiness(server, liveStatuses[server.id], rowBusy[server.id])
                    return (
                      <SelectItem key={server.id} value={server.id.toString()} disabled={!readiness.ready}>
                        <span className="flex min-w-0 flex-col">
                          <span>{server.name}</span>
                          <span className="text-xs text-muted-foreground">{addressForServer(server) || "无可用 IP"}</span>
                          <span className="flex gap-3 text-xs">
                            <span className={server.online ? "text-emerald-600" : "text-muted-foreground"}>● Agent {server.online ? "在线" : "离线"}</span>
                            <span className={readiness.ready ? "text-emerald-600" : "text-muted-foreground"}>● {readiness.label}</span>
                          </span>
                        </span>
                      </SelectItem>
                    )
                  })}
                </SelectContent>
              </Select>
              {selectedServer && !selectedReadiness?.ready && (
                <p className="text-xs text-destructive">{selectedReadiness?.label}，暂时不能创建代理节点。</p>
              )}
              {!nodes.length && <p className="text-xs text-muted-foreground">当前没有可选择的服务器。</p>}
            </div>
            <div className="space-y-2">
              <Label>连接地址</Label>
              <div className="flex flex-wrap gap-2">
                {([ ["ipv4", "IPv4"], ["ipv6", "IPv6"], ["custom", "自定义"] ] as const).map(([mode, label]) => (
                  <Button
                    key={mode}
                    type="button"
                    size="sm"
                    disabled={Boolean(submitStage)}
                    variant={form.addressMode === mode ? "default" : "outline"}
                    onClick={() => setForm((current) => ({ ...current, addressMode: mode }))}
                  >
                    {label}
                  </Button>
                ))}
              </div>
              {form.addressMode === "custom" ? (
                <Input
                  aria-label="自定义连接地址"
                  placeholder="例如 proxy.example.com"
                  disabled={Boolean(submitStage)}
                  value={form.customAddress}
                  onChange={(event) => setForm((current) => ({ ...current, customAddress: event.target.value }))}
                />
              ) : (
                <p className="text-xs text-muted-foreground">
                  {selectedServer
                    ? connectionAddress(form.addressMode, null, selectedServer) || "所选服务器没有该地址"
                    : "使用所属服务器的对应 IP 地址"}
                </p>
              )}
            </div>
            <div className="space-y-2">
              <Label htmlFor="proxy-name">代理节点名称</Label>
              <Input id="proxy-name" disabled={Boolean(submitStage)} value={form.name} onChange={(event) => setForm((current) => ({ ...current, name: event.target.value }))} placeholder="例如 HK Reality" />
            </div>
            <div className="space-y-2">
              <Label>协议</Label>
              <div><Badge variant="secondary">VLESS + Reality</Badge></div>
            </div>
            <div className="space-y-2">
              <Label>端口</Label>
              <Input value="自动分配" readOnly aria-label="端口自动分配" />
              <p className="text-xs text-muted-foreground">服务器会为此代理节点分配可用端口。</p>
            </div>
            <div className="space-y-3 rounded-md border p-3">
              <p className="text-sm font-medium">Reality 设置</p>
              <div className="space-y-2">
                <Label htmlFor="reality-sni">SNI</Label>
                <Input id="reality-sni" disabled={Boolean(submitStage)} value={form.realityServerName} onChange={(event) => setForm((current) => ({ ...current, realityServerName: event.target.value }))} required />
              </div>
              <div className="space-y-2">
                <Label htmlFor="reality-dest">Dest</Label>
                <Input id="reality-dest" disabled={Boolean(submitStage)} value={form.realityDest} onChange={(event) => setForm((current) => ({ ...current, realityDest: event.target.value }))} required />
              </div>
            </div>
            {formError && <p role="alert" className="break-words text-sm text-destructive">{formError}</p>}
            <DialogFooter>
              <Button type="button" variant="outline" disabled={Boolean(submitStage)} onClick={() => setCreateOpen(false)}>取消</Button>
              <Button type="submit" disabled={Boolean(submitStage) || !selectedReadiness?.ready}>
                {submitStage === "creating" ? <><LoaderCircle className="animate-spin" />创建中…</> : submitStage === "deploying" ? <><LoaderCircle className="animate-spin" />部署中…</> : "创建并部署"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </Card>
  )
}
