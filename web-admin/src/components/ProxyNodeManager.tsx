import { useCallback, useEffect, useRef, useState } from "react"
import { Copy, Download, LoaderCircle, Pencil, Plus, QrCode, RotateCw, Trash2 } from "lucide-react"
import { QRCodeSVG } from "qrcode.react"
import { toast } from "sonner"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Skeleton } from "@/components/ui/skeleton"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { api, type Node, type ProxyNode } from "@/lib/api"
import {
  createProxyNodeOperationGuard,
  deleteConfirmation,
  fetchProxyNodeShare,
  regenerateAndDeployProxyNode,
  regenerateConfirmation,
  removeProxyNode,
  proxyNodeShareDisabledReason,
  updateAndDeployProxyNode,
  proxyNodeUpdatePayload,
  type ProxyNodeCredential,
  type ProxyNodeShare,
  type ProxyNodeUpdatePayload,
} from "@/lib/proxy-node-actions"
import {
  importProxyNodeInbound,
  scanProxyNodeImports,
  type ProxyNodeImportCandidate,
  type ProxyNodeImportScan,
} from "@/lib/proxy-node-imports"

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
type EditProxyNodeForm = {
  name: string
  addressMode: AddressMode
  customAddress: string
  listenPort: string
  realityServerName: string
  realityDest: string
}
type ImportProxyNodeForm = {
  name: string
  addressMode: AddressMode
  customAddress: string
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

function DesiredStateBadge({ enabled }: { enabled: boolean }) {
  return enabled
    ? <Badge variant="outline" className="border-emerald-200 text-emerald-700 dark:border-emerald-900 dark:text-emerald-300">已启用</Badge>
    : <Badge variant="secondary">已停用</Badge>
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
  const [nodeOperations, setNodeOperations] = useState<Record<number, string>>({})
  const operationGuard = useRef(createProxyNodeOperationGuard())
  const [createOpen, setCreateOpen] = useState(false)
  const [form, setForm] = useState<NewProxyNodeForm>(EMPTY_FORM)
  const [formError, setFormError] = useState("")
  const [submitStage, setSubmitStage] = useState<SubmitStage | null>(null)
  const [editingNode, setEditingNode] = useState<ProxyNode | null>(null)
  const [editForm, setEditForm] = useState<EditProxyNodeForm | null>(null)
  const [editError, setEditError] = useState("")
  const [deleteTarget, setDeleteTarget] = useState<ProxyNode | null>(null)
  const [deleteError, setDeleteError] = useState("")
  const [regenerateTarget, setRegenerateTarget] = useState<{ node: ProxyNode; credential: ProxyNodeCredential } | null>(null)
  const [regenerateError, setRegenerateError] = useState("")
  const [sharingNodes, setSharingNodes] = useState<Record<number, boolean>>({})
  const [shareTarget, setShareTarget] = useState<{ node: ProxyNode; share: ProxyNodeShare | null } | null>(null)
  const shareRequestId = useRef(0)
  const [importOpen, setImportOpen] = useState(false)
  const [importServerId, setImportServerId] = useState("")
  const [importScan, setImportScan] = useState<ProxyNodeImportScan | null>(null)
  const [importTarget, setImportTarget] = useState<ProxyNodeImportCandidate | null>(null)
  const [importForm, setImportForm] = useState<ImportProxyNodeForm | null>(null)
  const [importLoading, setImportLoading] = useState(false)
  const [importing, setImporting] = useState(false)
  const [importError, setImportError] = useState("")
  const serversById = new Map(nodes.map((server) => [server.id, server]))
  const selectedServer = nodes.find((server) => server.id.toString() === form.nodeId)
  const selectedReadiness = selectedServer
    ? serverReadiness(selectedServer, liveStatuses[selectedServer.id], rowBusy[selectedServer.id])
    : null
  const selectedImportServer = nodes.find((server) => server.id.toString() === importServerId)
  const importConnection = importTarget && importForm
    ? connectionAddress(importForm.addressMode, importForm.customAddress, selectedImportServer)
    : ""

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

  function openImport() {
    setImportServerId(nodes[0]?.id.toString() || "")
    setImportScan(null)
    setImportTarget(null)
    setImportForm(null)
    setImportError("")
    setImportOpen(true)
  }

  async function scanExistingImports() {
    const nodeId = Number(importServerId)
    if (!Number.isInteger(nodeId) || nodeId <= 0) return
    setImportScan(null)
    setImportTarget(null)
    setImportForm(null)
    setImportError("")
    try {
      const result = await scanProxyNodeImports(api, nodeId, setImportLoading)
      setImportScan(result)
    } catch (cause) {
      setImportError((cause as Error).message)
    }
  }

  function selectImportTarget(candidate: ProxyNodeImportCandidate) {
    if (!candidate.importable) return
    setImportTarget(candidate)
    setImportForm({
      name: candidate.name,
      addressMode: candidate.suggested_address_mode || "custom",
      customAddress: "",
    })
    setImportError("")
  }

  async function confirmImport(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!importTarget || !importForm || !importScan || !selectedImportServer || importing) return
    if (!importForm.name.trim()) {
      setImportError("请填写代理节点名称")
      return
    }
    if (importForm.addressMode === "custom" && !importForm.customAddress.trim()) {
      setImportError("请填写自定义连接地址")
      return
    }
    if (importForm.addressMode !== "custom" && !importConnection) {
      setImportError(`所选服务器没有可用 ${importForm.addressMode === "ipv4" ? "IPv4" : "IPv6"} 地址`)
      return
    }

    setImportError("")
    try {
      await importProxyNodeInbound(api, selectedImportServer.id, {
        config_fingerprint: importScan.config_fingerprint,
        source_tag: importTarget.source_tag || "",
        name: importForm.name.trim(),
        address_mode: importForm.addressMode,
        custom_address: importForm.addressMode === "custom" ? importForm.customAddress.trim() : null,
      }, setImporting)
      toast.success("代理节点已导入并接管配置")
      setImportOpen(false)
      setImportTarget(null)
      setImportForm(null)
      const refreshed = await refresh()
      if (!refreshed) toast.error("导入成功，但代理节点列表刷新失败")
    } catch (cause) {
      const message = (cause as Error).message
      setImportError(message)
      toast.error(`导入代理节点失败：${message}`)
      await refresh()
    }
  }

  function beginNodeOperation(id: number, operation: string) {
    if (!operationGuard.current.tryStart(id)) return false
    setNodeOperations((current) => ({ ...current, [id]: operation }))
    return true
  }

  function finishNodeOperation(id: number) {
    operationGuard.current.finish(id)
    setNodeOperations((current) => {
      const next = { ...current }
      delete next[id]
      return next
    })
  }

  function setDesiredUpdate(id: number, update: ProxyNodeUpdatePayload) {
    setProxyNodes((current) => current.map((node) => node.id === id
      ? { ...node, ...update, deploy_status: "deploying", last_error: null }
      : node))
  }

  function setServerDeployment(nodeId: number, status: "deploying" | "deployed" | "failed", lastError: string | null = null) {
    setProxyNodes((current) => current.map((node) => node.node_id === nodeId
      ? { ...node, deploy_status: status, last_error: lastError }
      : node))
  }

  function openEdit(node: ProxyNode) {
    setEditingNode(node)
    setEditError("")
    setEditForm({
      name: node.name,
      addressMode: node.address_mode as AddressMode,
      customAddress: node.custom_address || "",
      listenPort: String(node.listen_port),
      realityServerName: node.reality_server_name,
      realityDest: node.reality_dest,
    })
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

  async function redeploy(proxyNode: ProxyNode) {
    const { id, node_id: nodeId } = proxyNode
    if (deploying[nodeId] || !beginNodeOperation(id, "deploying")) return
    setDeploying((current) => ({ ...current, [nodeId]: true }))
    setServerDeployment(nodeId, "deploying")
    let deployed = false
    try {
      await api(`/proxy/servers/${nodeId}/deploy`, { method: "POST" })
      deployed = true
      setServerDeployment(nodeId, "deployed")
      toast.success("代理节点已重新部署")
    } catch (cause) {
      setServerDeployment(nodeId, "failed", (cause as Error).message)
      toast.error(`代理节点重新部署失败：${(cause as Error).message}`)
    } finally {
      const refreshed = await refresh()
      if (!refreshed && deployed) toast.error("部署成功，但代理节点列表刷新失败")
      setDeploying((current) => {
        const next = { ...current }
        delete next[nodeId]
        return next
      })
      finishNodeOperation(id)
    }
  }

  async function applyNodeUpdate(
    node: ProxyNode,
    update: ProxyNodeUpdatePayload,
    successMessage: string,
    savedMessage: string,
    onUpdated?: () => void,
  ) {
    if (!beginNodeOperation(node.id, "saving")) return
    let saved = false
    try {
      await updateAndDeployProxyNode(api, node.id, node.node_id, update, () => {
        saved = true
        setServerDeployment(node.node_id, "deploying")
        setDesiredUpdate(node.id, update)
        onUpdated?.()
      }, () => setServerDeployment(node.node_id, "deployed"))
      toast.success(successMessage)
    } catch (cause) {
      const message = (cause as Error).message
      if (saved) {
        setServerDeployment(node.node_id, "failed", message)
        toast.error(`${savedMessage}，但部署失败：${message}`)
      } else {
        if (editingNode?.id === node.id) setEditError(message)
        toast.error(`代理节点保存失败：${message}`)
      }
    } finally {
      const refreshed = await refresh()
      if (!refreshed) toast.error("代理节点列表刷新失败")
      finishNodeOperation(node.id)
    }
  }

  async function saveEdit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!editingNode || !editForm) return
    setEditError("")
    const listenPort = Number(editForm.listenPort)
    if (!editForm.name.trim()) return setEditError("请填写代理节点名称")
    if (!Number.isInteger(listenPort) || listenPort < 1 || listenPort > 65535) {
      return setEditError("端口必须是 1 到 65535 之间的整数")
    }
    if (editForm.addressMode === "custom" && !editForm.customAddress.trim()) {
      return setEditError("请填写自定义连接地址")
    }
    const update = proxyNodeUpdatePayload(editingNode, {
      name: editForm.name.trim(),
      address_mode: editForm.addressMode,
      custom_address: editForm.addressMode === "custom" ? editForm.customAddress.trim() : null,
      listen_port: listenPort,
      reality_server_name: editForm.realityServerName.trim(),
      reality_dest: editForm.realityDest.trim(),
    })
    await applyNodeUpdate(editingNode, update, "代理节点已保存并部署", "配置已保存", () => {
      setEditingNode(null)
      setEditForm(null)
    })
  }

  async function toggleEnabled(node: ProxyNode, enabled: boolean) {
    const update = proxyNodeUpdatePayload(node, { enabled })
    await applyNodeUpdate(
      node,
      update,
      enabled ? "代理节点已启用" : "代理节点已停用",
      "启停状态已保存",
    )
  }

  function setShareBusy(id: number, busy: boolean) {
    setSharingNodes((current) => {
      const next = { ...current }
      if (busy) next[id] = true
      else delete next[id]
      return next
    })
  }

  async function copyShareUri(uri: string, nodeId: number) {
    setShareBusy(nodeId, true)
    try {
      if (!navigator.clipboard?.writeText) throw new Error("当前浏览器不支持剪贴板访问")
      await navigator.clipboard.writeText(uri)
      toast.success("代理链接已复制")
    } catch {
      toast.error("复制失败")
    } finally {
      setShareBusy(nodeId, false)
    }
  }

  async function copyShare(proxyNode: ProxyNode) {
    setShareBusy(proxyNode.id, true)
    let share: ProxyNodeShare
    try {
      share = await fetchProxyNodeShare(api, proxyNode.id)
    } catch (cause) {
      toast.error(`获取代理链接失败：${(cause as Error).message}`)
      setShareBusy(proxyNode.id, false)
      return
    }
    try {
      if (!navigator.clipboard?.writeText) throw new Error("当前浏览器不支持剪贴板访问")
      await navigator.clipboard.writeText(share.uri)
      toast.success("代理链接已复制")
    } catch {
      toast.error("复制失败")
    } finally {
      setShareBusy(proxyNode.id, false)
    }
  }

  async function openShareQr(proxyNode: ProxyNode) {
    const requestId = ++shareRequestId.current
    setShareTarget({ node: proxyNode, share: null })
    try {
      const share = await fetchProxyNodeShare(api, proxyNode.id, (busy) => setShareBusy(proxyNode.id, busy))
      if (shareRequestId.current === requestId) setShareTarget({ node: proxyNode, share })
    } catch (cause) {
      if (shareRequestId.current === requestId) {
        setShareTarget(null)
        toast.error(`获取代理链接失败：${(cause as Error).message}`)
      }
    }
  }

  async function confirmDelete() {
    if (!deleteTarget || !beginNodeOperation(deleteTarget.id, "deleting")) return
    const node = deleteTarget
    setDeleteError("")
    try {
      await removeProxyNode(api, node.id)
      toast.success("代理节点已删除")
      setDeleteTarget(null)
      const refreshed = await refresh()
      if (!refreshed) toast.error("代理节点列表刷新失败")
    } catch (cause) {
      const message = (cause as Error).message
      setDeleteError(message)
      toast.error(`删除代理节点失败：${message}`)
    } finally {
      finishNodeOperation(node.id)
    }
  }

  async function confirmRegenerate() {
    if (!regenerateTarget || !beginNodeOperation(regenerateTarget.node.id, "regenerating")) return
    const { node, credential } = regenerateTarget
    const prompt = regenerateConfirmation(credential, node.name)
    setRegenerateError("")
    let regenerated = false
    try {
      await regenerateAndDeployProxyNode(api, node.id, node.node_id, credential, (updated) => {
        regenerated = true
        setServerDeployment(node.node_id, "deploying")
        setProxyNodes((current) => current.map((item) => item.id === updated.id
          ? { ...updated, deploy_status: "deploying", last_error: null }
          : item))
        setEditingNode(updated)
        setRegenerateTarget(null)
      }, () => setServerDeployment(node.node_id, "deployed"))
      toast.success("凭据已重新生成并部署")
    } catch (cause) {
      const message = (cause as Error).message
      if (regenerated) {
        setServerDeployment(node.node_id, "failed", message)
        setEditingNode((current) => current?.id === node.id
          ? { ...current, deploy_status: "failed", last_error: message }
          : current)
        toast.error(`凭据已重新生成，但部署失败：${message}`)
        setRegenerateTarget(null)
      } else {
        setRegenerateError(message)
        toast.error(`${prompt.action}失败：${message}`)
      }
    } finally {
      const refreshed = await refresh()
      if (refreshed) {
        const current = refreshed.find((item) => item.id === node.id)
        if (current) setEditingNode((item) => item?.id === node.id ? current : item)
      } else {
        toast.error("代理节点列表刷新失败")
      }
      finishNodeOperation(node.id)
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
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" onClick={openImport}><Download />导入现有配置</Button>
            <Button onClick={openCreate}><Plus />新建代理节点</Button>
          </div>
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
          <div className="mt-4 flex justify-center gap-2">
            <Button variant="outline" onClick={openImport}><Download />导入现有配置</Button>
            <Button onClick={openCreate}><Plus />新建代理节点</Button>
          </div>
        </div>
      ) : proxyNodes.length > 0 ? (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>启用</TableHead>
              <TableHead>所属服务器</TableHead>
              <TableHead>代理节点名称</TableHead>
              <TableHead>协议</TableHead>
              <TableHead>连接地址</TableHead>
              <TableHead>端口</TableHead>
              <TableHead>SNI</TableHead>
              <TableHead>期望状态</TableHead>
              <TableHead>部署状态</TableHead>
              <TableHead>操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {proxyNodes.map((proxyNode) => {
              const server = serversById.get(proxyNode.node_id)
              const serverBusy = Boolean(deploying[proxyNode.node_id])
              const nodeBusy = Boolean(nodeOperations[proxyNode.id])
              const shareDisabledReason = proxyNodeShareDisabledReason(
                proxyNode,
                connectionAddress(proxyNode.address_mode, proxyNode.custom_address, server),
              )
              const shareDisabledMessage = shareDisabledReason || (nodeBusy ? "代理节点操作中，暂不可分享" : null)
              const shareDisabled = nodeBusy || Boolean(shareDisabledReason) || Boolean(sharingNodes[proxyNode.id])
              return (
                <TableRow key={proxyNode.id}>
                  <TableCell>
                    <Switch
                      aria-label={`${proxyNode.enabled ? "停用" : "启用"} ${proxyNode.name}`}
                      checked={proxyNode.enabled}
                      disabled={nodeBusy}
                      onCheckedChange={(enabled) => void toggleEnabled(proxyNode, enabled)}
                    />
                  </TableCell>
                  <TableCell className="font-medium">{server?.name || `服务器 #${proxyNode.node_id}`}</TableCell>
                  <TableCell>{proxyNode.name}</TableCell>
                  <TableCell><Badge variant="secondary">{protocolLabel(proxyNode.protocol)}</Badge></TableCell>
                  <TableCell className="max-w-52 truncate" title={connectionAddress(proxyNode.address_mode, proxyNode.custom_address, server) || undefined}>
                    {connectionAddress(proxyNode.address_mode, proxyNode.custom_address, server) || "—"}
                  </TableCell>
                  <TableCell className="tnum">{proxyNode.listen_port}</TableCell>
                  <TableCell>{proxyNode.reality_server_name || "—"}</TableCell>
                  <TableCell><DesiredStateBadge enabled={proxyNode.enabled} /></TableCell>
                  <TableCell><DeploymentBadge proxyNode={proxyNode} /></TableCell>
                  <TableCell className="min-w-96">
                    <div className="flex flex-wrap gap-2">
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={shareDisabled}
                        title={shareDisabledMessage || undefined}
                        onClick={() => void copyShare(proxyNode)}
                      >
                        {sharingNodes[proxyNode.id] ? <LoaderCircle className="animate-spin" /> : <Copy />}
                        复制链接
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={shareDisabled}
                        title={shareDisabledMessage || undefined}
                        onClick={() => void openShareQr(proxyNode)}
                      >
                        <QrCode />二维码
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={nodeBusy}
                        onClick={() => openEdit(proxyNode)}
                      >
                        <Pencil />编辑
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={serverBusy || nodeBusy}
                        onClick={() => void redeploy(proxyNode)}
                      >
                        <RotateCw className={serverBusy ? "animate-spin" : undefined} />
                        {serverBusy ? "部署中…" : "重新部署"}
                      </Button>
                      <Button
                        variant="destructive"
                        size="sm"
                        disabled={nodeBusy}
                        onClick={() => { setDeleteError(""); setDeleteTarget(proxyNode) }}
                      >
                        <Trash2 />删除
                      </Button>
                    </div>
                    {shareDisabledMessage && <p className="mt-1 text-xs text-muted-foreground">{shareDisabledMessage}</p>}
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
      </Table>
      ) : null}
      <Dialog
        open={importOpen}
        onOpenChange={(open) => {
          if (!importing) setImportOpen(open)
        }}
      >
        <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>导入服务器已有配置</DialogTitle>
            <DialogDescription>扫描 sing-box 中未由 Monitor 管理的 inbound。确认后会校验并应用接管配置。</DialogDescription>
          </DialogHeader>
          <div className="space-y-2">
            <Label htmlFor="proxy-import-server">所属服务器</Label>
            <div className="flex gap-2">
              <Select
                value={importServerId}
                disabled={importing || importLoading}
                onValueChange={(value) => {
                  setImportServerId(value)
                  setImportScan(null)
                  setImportTarget(null)
                  setImportForm(null)
                  setImportError("")
                }}
              >
                <SelectTrigger id="proxy-import-server" className="min-w-0 flex-1"><SelectValue placeholder="选择服务器" /></SelectTrigger>
                <SelectContent>
                  {nodes.map((server) => (
                    <SelectItem key={server.id} value={server.id.toString()}>
                      {server.name} · {addressForServer(server) || "无可用 IP"}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button type="button" variant="outline" disabled={!importServerId || importLoading || importing} onClick={() => void scanExistingImports()}>
                {importLoading ? <><LoaderCircle className="animate-spin" />扫描中…</> : "扫描"}
              </Button>
            </div>
          </div>
          {importError && <p role="alert" className="break-words text-sm text-destructive">{importError}</p>}
          {importScan && (
            <div className="space-y-3">
              <p className="text-sm text-muted-foreground">
                发现 {importScan.inbounds.length} 个未由 Monitor 管理的 inbound。确认后原 tag 会被替换为 Monitor 管理的 tag。
              </p>
              {importScan.inbounds.length === 0 ? (
                <div className="rounded-md border border-dashed p-4 text-center text-sm text-muted-foreground">没有发现可扫描的 inbound。</div>
              ) : (
                <div className="max-h-64 space-y-2 overflow-y-auto pr-1">
                  {importScan.inbounds.map((candidate, index) => (
                    <div key={`${candidate.source_tag || "untagged"}-${index}`} className="rounded-md border p-3">
                      <div className="flex flex-wrap items-start justify-between gap-3">
                        <div className="min-w-0 space-y-1">
                          <p className="font-medium">{candidate.source_tag || "无 tag"}</p>
                          <p className="text-sm text-muted-foreground">
                            {candidate.protocol} · {candidate.listen_port ? `端口 ${candidate.listen_port}` : "无有效端口"}
                            {candidate.reality_server_name ? ` · SNI ${candidate.reality_server_name}` : ""}
                          </p>
                          <p className={candidate.importable ? "text-xs text-emerald-700 dark:text-emerald-300" : "text-xs text-destructive"}>
                            {candidate.importable ? "可导入" : candidate.reason || "不可导入"}
                          </p>
                        </div>
                        {candidate.importable && (
                          <Button type="button" size="sm" variant={importTarget?.source_tag === candidate.source_tag ? "default" : "outline"} disabled={importing} onClick={() => selectImportTarget(candidate)}>
                            选择导入
                          </Button>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
          {importTarget && importForm && (
            <form className="space-y-4 rounded-md border p-4" onSubmit={(event) => void confirmImport(event)}>
              <div className="space-y-1">
                <p className="font-medium">确认接管「{importTarget.source_tag}」</p>
                <p className="text-sm text-muted-foreground">Monitor 会先校验生成配置，再替换服务器上的原 inbound。校验或明确应用失败时会保留原配置。</p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="proxy-import-name">代理节点名称</Label>
                <Input id="proxy-import-name" value={importForm.name} disabled={importing} onChange={(event) => setImportForm((current) => current ? { ...current, name: event.target.value } : current)} required />
              </div>
              <div className="space-y-2">
                <Label>客户端连接地址</Label>
                <Select
                  value={importForm.addressMode}
                  disabled={importing}
                  onValueChange={(addressMode: AddressMode) => setImportForm((current) => current ? { ...current, addressMode } : current)}
                >
                  <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="ipv4" disabled={!connectionAddress("ipv4", null, selectedImportServer)}>IPv4</SelectItem>
                    <SelectItem value="ipv6" disabled={!connectionAddress("ipv6", null, selectedImportServer)}>IPv6</SelectItem>
                    <SelectItem value="custom">自定义</SelectItem>
                  </SelectContent>
                </Select>
                {importForm.addressMode === "custom" ? (
                  <Input
                    aria-label="导入节点自定义连接地址"
                    placeholder="例如 proxy.example.com"
                    value={importForm.customAddress}
                    disabled={importing}
                    onChange={(event) => setImportForm((current) => current ? { ...current, customAddress: event.target.value } : current)}
                  />
                ) : (
                  <p className="text-xs text-muted-foreground">{importConnection || "所选服务器没有该地址"}</p>
                )}
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="space-y-1"><Label>服务器监听地址</Label><Input value={importTarget.listen_address || "—"} readOnly /></div>
                <div className="space-y-1"><Label>端口</Label><Input value={importTarget.listen_port?.toString() || "—"} readOnly /></div>
                <div className="space-y-1"><Label>SNI</Label><Input value={importTarget.reality_server_name || "—"} readOnly /></div>
                <div className="space-y-1"><Label>Dest</Label><Input value={importTarget.reality_dest || "—"} readOnly /></div>
              </div>
              <div className="space-y-1 rounded-md bg-muted/50 p-3 text-xs">
                <p>UUID：{importTarget.uuid ? `${importTarget.uuid.slice(0, 8)}…` : "—"}</p>
                <p className="break-all">Reality Public Key：{importTarget.reality_public_key || "—"}</p>
                <p>Short ID：{importTarget.reality_short_id || "—"}</p>
                <p className="text-muted-foreground">服务器监听地址与客户端连接地址分别保存；Private Key 不会显示在浏览器。</p>
              </div>
              <DialogFooter>
                <Button type="button" variant="outline" disabled={importing} onClick={() => { setImportTarget(null); setImportForm(null) }}>返回列表</Button>
                <Button type="submit" disabled={importing || !importForm.name.trim() || !importTarget.source_tag || !importConnection}>
                  {importing ? <><LoaderCircle className="animate-spin" />正在校验并接管…</> : "确认导入并接管"}
                </Button>
              </DialogFooter>
            </form>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" disabled={importing} onClick={() => setImportOpen(false)}>关闭</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={Boolean(shareTarget)}
        onOpenChange={(open) => {
          if (!open) {
            shareRequestId.current += 1
            setShareTarget(null)
          }
        }}
      >
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{shareTarget?.node.name || "代理节点二维码"}</DialogTitle>
            <DialogDescription>扫描二维码导入 VLESS + Reality 节点</DialogDescription>
          </DialogHeader>
          {shareTarget && !shareTarget.share ? (
            <div className="flex flex-col items-center gap-3 py-8" aria-label="正在生成二维码">
              <LoaderCircle className="size-8 animate-spin text-muted-foreground" />
              <p className="text-sm text-muted-foreground">正在加载分享链接…</p>
            </div>
          ) : shareTarget?.share ? (
            <div className="flex flex-col items-center gap-4">
              <div className="rounded-lg bg-white p-3">
                <QRCodeSVG value={shareTarget.share.uri} size={224} level="M" title={`${shareTarget.share.name} VLESS Reality 二维码`} />
              </div>
              <div className="space-y-1 text-center">
                <p className="text-sm font-medium">VLESS + Reality</p>
                <p className="text-sm text-muted-foreground">{shareTarget.share.address}</p>
              </div>
              <Button
                className="w-full"
                disabled={Boolean(sharingNodes[shareTarget.node.id])}
                onClick={() => void copyShareUri(shareTarget.share?.uri || "", shareTarget.node.id)}
              >
                <Copy />复制链接
              </Button>
              <details className="w-full text-sm">
                <summary className="cursor-pointer text-muted-foreground">显示完整链接</summary>
                <code className="mt-2 block max-h-28 overflow-auto break-all rounded-md bg-muted p-3 text-xs">{shareTarget.share.uri}</code>
              </details>
            </div>
          ) : null}
        </DialogContent>
      </Dialog>
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
      <Dialog
        open={Boolean(editingNode && editForm)}
        onOpenChange={(open) => {
          if (open) return
          if (editingNode && nodeOperations[editingNode.id]) return
          setEditingNode(null)
          setEditForm(null)
        }}
      >
        {editingNode && editForm && (
          <DialogContent className="sm:max-w-2xl">
            <DialogHeader>
              <DialogTitle>编辑「{editingNode.name}」</DialogTitle>
              <DialogDescription>修改后会保存期望配置并部署到所属服务器。</DialogDescription>
            </DialogHeader>
            <form className="space-y-4" onSubmit={(event) => void saveEdit(event)}>
              <div className="space-y-2">
                <Label htmlFor="proxy-edit-name">代理节点名称</Label>
                <Input
                  id="proxy-edit-name"
                  value={editForm.name}
                  disabled={Boolean(nodeOperations[editingNode.id])}
                  onChange={(event) => setEditForm((current) => current ? { ...current, name: event.target.value } : current)}
                  required
                />
              </div>
              <div className="space-y-2">
                <Label>连接地址模式</Label>
                <Select
                  value={editForm.addressMode}
                  disabled={Boolean(nodeOperations[editingNode.id])}
                  onValueChange={(addressMode: AddressMode) => setEditForm((current) => current ? { ...current, addressMode } : current)}
                >
                  <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="ipv4">IPv4</SelectItem>
                    <SelectItem value="ipv6">IPv6</SelectItem>
                    <SelectItem value="custom">自定义</SelectItem>
                  </SelectContent>
                </Select>
                {editForm.addressMode === "custom" ? (
                  <Input
                    aria-label="自定义连接地址"
                    placeholder="例如 proxy.example.com"
                    value={editForm.customAddress}
                    disabled={Boolean(nodeOperations[editingNode.id])}
                    onChange={(event) => setEditForm((current) => current ? { ...current, customAddress: event.target.value } : current)}
                    required
                  />
                ) : (
                  <p className="text-xs text-muted-foreground">
                    {connectionAddress(editForm.addressMode, null, serversById.get(editingNode.node_id)) || "使用所属服务器的对应 IP 地址"}
                  </p>
                )}
              </div>
              <div className="space-y-2">
                <Label htmlFor="proxy-edit-port">端口</Label>
                <Input
                  id="proxy-edit-port"
                  type="number"
                  min={1}
                  max={65535}
                  step={1}
                  value={editForm.listenPort}
                  disabled={Boolean(nodeOperations[editingNode.id])}
                  onChange={(event) => setEditForm((current) => current ? { ...current, listenPort: event.target.value } : current)}
                  required
                />
              </div>
              <div className="space-y-3 rounded-md border p-3">
                <p className="text-sm font-medium">Reality 设置</p>
                <div className="space-y-2">
                  <Label htmlFor="proxy-edit-sni">SNI</Label>
                  <Input
                    id="proxy-edit-sni"
                    value={editForm.realityServerName}
                    disabled={Boolean(nodeOperations[editingNode.id])}
                    onChange={(event) => setEditForm((current) => current ? { ...current, realityServerName: event.target.value } : current)}
                    required
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="proxy-edit-dest">Dest</Label>
                  <Input
                    id="proxy-edit-dest"
                    value={editForm.realityDest}
                    disabled={Boolean(nodeOperations[editingNode.id])}
                    onChange={(event) => setEditForm((current) => current ? { ...current, realityDest: event.target.value } : current)}
                    required
                  />
                </div>
              </div>
              <div className="space-y-3 rounded-md border p-3">
                <div>
                  <p className="text-sm font-medium">高级配置</p>
                  <p className="text-xs text-muted-foreground">凭据只读；重新生成后，使用旧凭据的客户端配置会失效。</p>
                </div>
                <CredentialRow label="UUID" value={editingNode.uuid} disabled={Boolean(nodeOperations[editingNode.id])} onRegenerate={() => {
                  setRegenerateError("")
                  setRegenerateTarget({ node: editingNode, credential: "uuid" })
                }} />
                <CredentialRow label="Reality Public Key" value={editingNode.reality_public_key} disabled={Boolean(nodeOperations[editingNode.id])} onRegenerate={() => {
                  setRegenerateError("")
                  setRegenerateTarget({ node: editingNode, credential: "reality_key" })
                }} regenerateLabel="重新生成 Reality 密钥" />
                <CredentialRow label="Short ID" value={editingNode.reality_short_id} disabled={Boolean(nodeOperations[editingNode.id])} onRegenerate={() => {
                  setRegenerateError("")
                  setRegenerateTarget({ node: editingNode, credential: "short_id" })
                }} />
              </div>
              {editError && <p role="alert" className="break-words text-sm text-destructive">保存失败：{editError}</p>}
              <DialogFooter>
                <Button type="button" variant="outline" disabled={Boolean(nodeOperations[editingNode.id])} onClick={() => { setEditingNode(null); setEditForm(null) }}>取消</Button>
                <Button type="submit" disabled={Boolean(nodeOperations[editingNode.id])}>
                  {nodeOperations[editingNode.id] === "saving"
                    ? <><LoaderCircle className="animate-spin" />保存并部署中…</>
                    : "保存并部署"}
                </Button>
              </DialogFooter>
            </form>
          </DialogContent>
        )}
      </Dialog>
      <Dialog
        open={Boolean(deleteTarget)}
        onOpenChange={(open) => {
          if (!open && (!deleteTarget || !nodeOperations[deleteTarget.id])) setDeleteTarget(null)
        }}
      >
        {deleteTarget && (() => {
          const prompt = deleteConfirmation(deleteTarget.name)
          const busy = nodeOperations[deleteTarget.id] === "deleting"
          return (
            <DialogContent className="sm:max-w-md">
              <DialogHeader>
                <DialogTitle>{prompt.title}</DialogTitle>
                <DialogDescription className="leading-relaxed">{prompt.description}</DialogDescription>
              </DialogHeader>
              {deleteError && <p role="alert" className="break-words text-sm text-destructive">删除失败：{deleteError}</p>}
              <DialogFooter>
                <Button variant="outline" disabled={busy} onClick={() => setDeleteTarget(null)}>取消</Button>
                <Button variant="destructive" disabled={busy} onClick={() => void confirmDelete()}>
                  {busy ? <><LoaderCircle className="animate-spin" />正在移除配置…</> : prompt.action}
                </Button>
              </DialogFooter>
            </DialogContent>
          )
        })()}
      </Dialog>
      <Dialog
        open={Boolean(regenerateTarget)}
        onOpenChange={(open) => {
          if (!open && (!regenerateTarget || !nodeOperations[regenerateTarget.node.id])) setRegenerateTarget(null)
        }}
      >
        {regenerateTarget && (() => {
          const prompt = regenerateConfirmation(regenerateTarget.credential, regenerateTarget.node.name)
          const busy = nodeOperations[regenerateTarget.node.id] === "regenerating"
          return (
            <DialogContent className="sm:max-w-md">
              <DialogHeader>
                <DialogTitle>{prompt.title}</DialogTitle>
                <DialogDescription className="leading-relaxed">{prompt.description}</DialogDescription>
              </DialogHeader>
              {regenerateError && <p role="alert" className="break-words text-sm text-destructive">重新生成失败：{regenerateError}</p>}
              <DialogFooter>
                <Button variant="outline" disabled={busy} onClick={() => setRegenerateTarget(null)}>取消</Button>
                <Button variant="destructive" disabled={busy} onClick={() => void confirmRegenerate()}>
                  {busy ? <><LoaderCircle className="animate-spin" />重新生成并部署中…</> : prompt.action}
                </Button>
              </DialogFooter>
            </DialogContent>
          )
        })()}
      </Dialog>
    </Card>
  )
}

function CredentialRow({
  label,
  value,
  disabled,
  onRegenerate,
  regenerateLabel = `重新生成 ${label}`,
}: {
  label: string
  value: string
  disabled: boolean
  onRegenerate: () => void
  regenerateLabel?: string
}) {
  return (
    <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
      <div className="min-w-0 space-y-1">
        <Label>{label}</Label>
        <code className="block break-all text-xs text-muted-foreground">{value}</code>
      </div>
      <Button type="button" variant="outline" size="sm" disabled={disabled} onClick={onRegenerate}>{regenerateLabel}</Button>
    </div>
  )
}
