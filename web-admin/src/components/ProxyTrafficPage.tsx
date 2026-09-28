import { useEffect, useRef, useState } from "react"
import { Activity, Download, RefreshCw, Upload, Users } from "lucide-react"

import { AdminConfirmDialog } from "@/components/AdminShared"
import { TrafficUsage } from "@/components/TrafficUsage"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { ProxyProtocolBadge } from "@/components/ProxyBadges"
import type { Node } from "@/lib/api"
import { toast } from "sonner"
import { displayProxyName } from "@/lib/proxy-name"
import { bytes } from "@/lib/format"
import {
  resetProxyUserTraffic,
  resetProxyNodeTraffic,
  getProxyTrafficOverview,
  listProxyTrafficNodeRows,
  listProxyUserTraffic,
  type ProxyTrafficNodeRow,
  type ProxyTrafficOverview,
  type ProxyTrafficListOptions,
} from "@/lib/resources"

const NODE_PAGE_SIZE = 100
const REFRESH_MS = 5_000
type UserTrafficRow = Awaited<ReturnType<typeof listProxyUserTraffic>>[number]

async function listAllProxyTrafficNodeRows(options: Omit<ProxyTrafficListOptions, "page" | "page_size">): Promise<ProxyTrafficNodeRow[]> {
  const first = await listProxyTrafficNodeRows({ ...options, page: 1, page_size: NODE_PAGE_SIZE })
  const pageCount = Math.ceil(first.total / NODE_PAGE_SIZE)
  if (pageCount <= 1) return first.items

  const rest = await Promise.all(Array.from({ length: pageCount - 1 }, (_, index) =>
    listProxyTrafficNodeRows({ ...options, page: index + 2, page_size: NODE_PAGE_SIZE }),
  ))
  return [...first.items, ...rest.flatMap((page) => page.items)]
}

function StatCard({ label, value, icon: Icon }: { label: string; value: string; icon: typeof Upload }) {
  return <Card className="gap-2 p-4">
    <div className="flex items-center gap-2 text-sm text-muted-foreground"><Icon className="size-4" />{label}</div>
    <div className="tnum text-xl font-semibold tracking-tight">{value}</div>
  </Card>
}

export function ProxyTrafficPage({ nodes }: { nodes: Node[] }) {
  const [overview, setOverview] = useState<ProxyTrafficOverview | null>(null)
  const [userRows, setUserRows] = useState<UserTrafficRow[] | null>(null)
  const [nodeRows, setNodeRows] = useState<ProxyTrafficNodeRow[] | null>(null)
  const [error, setError] = useState("")
  const [loading, setLoading] = useState(true)
  const [refreshKey, setRefreshKey] = useState(0)
  const loaded = useRef(false)
  const [resetTarget, setResetTarget] = useState<{ kind: "user" | "node"; id: number; name: string } | null>(null)
  const [resetting, setResetting] = useState(false)
  const resettingRef = useRef(false)
  const generation = useRef(0)

  async function resetTraffic() {
    if (!resetTarget || resettingRef.current) return
    resettingRef.current = true
    setResetting(true)
    generation.current += 1
    try {
      if (resetTarget.kind === "user") await resetProxyUserTraffic(resetTarget.id)
      else await resetProxyNodeTraffic(resetTarget.id)
      toast.success("流量已重置")
      setResetTarget(null)
    } catch (cause) {
      toast.error((cause as Error).message || "重置流量失败")
    } finally {
      resettingRef.current = false
      setResetting(false)
      setRefreshKey((value) => value + 1)
    }
  }

  useEffect(() => {
    let current = true
    let busy = false
    async function load() {
      if (busy || resettingRef.current) return
      busy = true
      const requestGeneration = generation.current
      if (!loaded.current) setLoading(true)
      try {
        const [nextOverview, nextUsers, nextNodes] = await Promise.all([
          getProxyTrafficOverview(),
          listProxyUserTraffic(),
          listAllProxyTrafficNodeRows({
            q: "",
            node_id: null,
            sort: "total",
            order: "desc",
          }),
        ])
        if (!current || requestGeneration !== generation.current) return
        setOverview(nextOverview)
        setUserRows(nextUsers)
        setNodeRows(nextNodes)
        setError("")
        loaded.current = true
      } catch (cause) {
        if (current && requestGeneration === generation.current) setError((cause as Error).message || "读取流量统计失败")
      } finally {
        busy = false
        if (current) setLoading(false)
      }
    }
    void load()
    const timer = window.setInterval(() => void load(), REFRESH_MS)
    return () => {
      current = false
      window.clearInterval(timer)
    }
  }, [refreshKey])

  const nodesById = new Map(nodes.map((node) => [node.id, node]))
  const users = [...(userRows ?? [])].sort((a, b) => (b.uplink_bytes + b.downlink_bytes) - (a.uplink_bytes + a.downlink_bytes)
    || (a.username ?? "").localeCompare(b.username ?? ""))

  return <div className="space-y-5">
    <div className="flex items-center gap-3">
      <div className="mr-auto">
        <h1 className="text-lg font-semibold">流量统计</h1>
        <p className="mt-1 text-xs text-muted-foreground">代理业务累计 · 自建立基线或上次清零以来</p>
      </div>
      <Button variant="outline" size="sm" onClick={() => setRefreshKey((value) => value + 1)} disabled={loading}>
        <RefreshCw className={loading ? "animate-spin" : ""} /> 刷新
      </Button>
    </div>

    {overview && <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">
      <StatCard label="总流量" value={bytes(overview.total_bytes)} icon={Activity} />
      <StatCard label="上传流量" value={bytes(overview.uplink_bytes)} icon={Upload} />
      <StatCard label="下载流量" value={bytes(overview.downlink_bytes)} icon={Download} />
      <StatCard label="最近采集用户" value={String(overview.active_users)} icon={Users} />
    </div>}
    {!overview && loading && <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">{Array.from({ length: 4 }, (_, i) => <Skeleton key={i} className="h-20" />)}</div>}

    <p className="text-xs text-muted-foreground">用户流量汇总所有服务器；代理流量按服务器统计启用代理的 inbound 流量，两种视角不会相加。代理进度以累计业务流量对比节点额度，两者的重置周期可能不同。</p>
    {error && <div className="flex flex-wrap items-center gap-2 text-sm text-destructive" role="alert">
      <span>读取流量统计失败：{error}</span>
      <Button variant="outline" size="sm" onClick={() => setRefreshKey((value) => value + 1)}>重试</Button>
    </div>}

    <section className="space-y-3">
      <h2 className="text-base font-semibold">用户流量</h2>
      <Card className="overflow-x-auto p-0">
        <Table className="min-w-[900px] table-fixed">
          <colgroup><col style={{ width: "26%" }} /><col style={{ width: "14%" }} /><col style={{ width: "14%" }} /><col style={{ width: "34%" }} /><col style={{ width: "12%" }} /></colgroup>
          <TableHeader className="bg-muted/50"><TableRow>
            <TableHead>用户名</TableHead><TableHead className="text-right">上传流量</TableHead>
            <TableHead className="text-right">下载流量</TableHead><TableHead>流量使用情况</TableHead><TableHead className="text-right">操作</TableHead>
          </TableRow></TableHeader>
          <TableBody>
            {users.map((row) => {
              const total = row.uplink_bytes + row.downlink_bytes
              const admin = row.username === "admin"
              return <TableRow key={`${row.user_id ?? row.username}`} className="h-12">
                <TableCell className="whitespace-normal"><div className="flex flex-wrap items-center gap-2 font-medium">
                  <span className="min-w-0 [overflow-wrap:anywhere]">{row.username ?? "未知用户"}</span>
                  <Badge variant={admin ? "default" : "secondary"} className="font-normal">{admin ? "管理员" : "普通用户"}</Badge>
                </div></TableCell>
                <TableCell className="tnum text-right">{bytes(row.uplink_bytes)}</TableCell>
                <TableCell className="tnum text-right">{bytes(row.downlink_bytes)}</TableCell>
                <TableCell className="min-w-64 py-2"><TrafficUsage used={total} limit={row.traffic_limit} /></TableCell>
                <TableCell className="text-right"><Button variant="ghost" size="sm" className="px-2" disabled={resetting || row.user_id === undefined} onClick={() => setResetTarget({ kind: "user", id: row.user_id!, name: row.username ?? "未知用户" })}>重置流量</Button></TableCell>
              </TableRow>
            })}
            {!loading && users.length === 0 && <TableRow><TableCell colSpan={5} className="py-10 text-center text-sm text-muted-foreground">尚无用户流量记录</TableCell></TableRow>}
            {loading && !userRows && <TableRow><TableCell colSpan={5} className="p-4"><Skeleton className="h-10 w-full" /></TableCell></TableRow>}
          </TableBody>
        </Table>
      </Card>
    </section>

    <section className="space-y-3">
      <h2 className="text-base font-semibold">代理流量</h2>
      <Card className="overflow-x-auto p-0">
        <Table className="min-w-[900px] table-fixed">
          <colgroup><col style={{ width: "26%" }} /><col style={{ width: "14%" }} /><col style={{ width: "14%" }} /><col style={{ width: "34%" }} /><col style={{ width: "12%" }} /></colgroup>
          <TableHeader className="bg-muted/50"><TableRow>
            <TableHead>代理</TableHead><TableHead className="text-right">上传流量</TableHead>
            <TableHead className="text-right">下载流量</TableHead><TableHead>流量使用情况</TableHead><TableHead className="text-right">操作</TableHead>
          </TableRow></TableHeader>
          <TableBody>
            {nodeRows?.map((row) => <TableRow key={row.node_id} className="h-12">
              <TableCell className="whitespace-normal"><div className="space-y-1.5">
                {row.proxies.length ? row.proxies.map((proxy) => <div key={proxy.id} className="flex flex-wrap items-center gap-2">
                  <span className={`min-w-0 font-medium [overflow-wrap:anywhere] ${proxy.enabled ? "" : "text-muted-foreground"}`} title={proxy.enabled ? undefined : "已停用"}>{displayProxyName({ ...proxy, node_id: row.node_id }, nodesById.get(row.node_id))}</span>
                  <ProxyProtocolBadge protocol={proxy.protocol} />
                </div>) : <span className="text-muted-foreground [overflow-wrap:anywhere]">{row.node_name}（无现存代理）</span>}
              </div></TableCell>
              <TableCell className="tnum text-right">{bytes(row.uplink_bytes)}</TableCell>
              <TableCell className="tnum text-right">{bytes(row.downlink_bytes)}</TableCell>
              <TableCell className="min-w-64 py-2"><TrafficUsage used={row.total_bytes} limit={nodesById.get(row.node_id)?.traffic_limit} /></TableCell>
              <TableCell className="text-right"><Button variant="ghost" size="sm" className="px-2" disabled={resetting} onClick={() => setResetTarget({ kind: "node", id: row.node_id, name: row.node_name })}>重置流量</Button></TableCell>
            </TableRow>)}
            {!loading && nodeRows?.length === 0 && <TableRow><TableCell colSpan={5} className="py-10 text-center text-sm text-muted-foreground">尚无代理节点流量记录</TableCell></TableRow>}
            {loading && !nodeRows && <TableRow><TableCell colSpan={5} className="p-4"><Skeleton className="h-10 w-full" /></TableCell></TableRow>}
          </TableBody>
        </Table>
      </Card>
    </section>
    {resetTarget && <AdminConfirmDialog
      title={`重置「${resetTarget.name}」的流量？`}
      description={resetTarget.kind === "user" ? "将清零该用户在所有节点的累计业务流量，月度重置计划保持不变。" : "将清零该节点所有代理汇总的累计业务流量，不会清零用户流量或节点本月计费用量。"}
      confirmLabel="重置流量"
      busy={resetting}
      onClose={() => { if (!resettingRef.current) setResetTarget(null) }}
      onConfirm={() => void resetTraffic()}
    />}
  </div>
}
