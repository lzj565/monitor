import { useEffect, useRef, useState } from "react"
import { Activity, Download, RefreshCw, Upload, Users } from "lucide-react"

import { AdminSearchInput } from "@/components/AdminShared"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent } from "@/components/ui/card"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Skeleton } from "@/components/ui/skeleton"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { ProxyProtocolBadge } from "@/components/ProxyBadges"
import type { Node } from "@/lib/api"
import { bytes, relativeTime } from "@/lib/format"
import {
  getProxyTrafficOverview,
  listProxyTrafficNodeRows,
  listProxyTrafficUserRows,
  type ProxyTrafficNodeRow,
  type ProxyTrafficOverview,
  type ProxyTrafficUserRow,
} from "@/lib/resources"

type Tab = "users" | "nodes"
type Sort = "total_desc" | "total_asc" | "uplink_desc" | "uplink_asc" | "downlink_desc" | "downlink_asc"

const PAGE_SIZE = 25
const REFRESH_MS = 5_000

function sortOptions(value: Sort): { sort: "total" | "uplink" | "downlink"; order: "asc" | "desc" } {
  const [sort, order] = value.split("_")
  return {
    sort: sort === "uplink" || sort === "downlink" ? sort : "total",
    order: order === "asc" ? "asc" : "desc",
  }
}

function StatCard({ label, value, icon: Icon }: { label: string; value: string; icon: typeof Upload }) {
  return <Card className="gap-2 p-4">
    <div className="flex items-center gap-2 text-sm text-muted-foreground"><Icon className="size-4" />{label}</div>
    <div className="tnum text-xl font-semibold tracking-tight">{value}</div>
  </Card>
}

export function ProxyTrafficPage({ nodes }: { nodes: Node[] }) {
  const [tab, setTab] = useState<Tab>("users")
  const [query, setQuery] = useState("")
  const [debouncedQuery, setDebouncedQuery] = useState("")
  const [nodeId, setNodeId] = useState("all")
  const [sort, setSort] = useState<Sort>("total_desc")
  const [page, setPage] = useState(1)
  const [overview, setOverview] = useState<ProxyTrafficOverview | null>(null)
  const [userPage, setUserPage] = useState<{ items: ProxyTrafficUserRow[]; total: number } | null>(null)
  const [nodePage, setNodePage] = useState<{ items: ProxyTrafficNodeRow[]; total: number } | null>(null)
  const [error, setError] = useState("")
  const [loading, setLoading] = useState(true)
  const [now, setNow] = useState(Date.now())
  const [refreshKey, setRefreshKey] = useState(0)
  const loaded = useRef(false)
  const busy = useRef(false)
  const { sort: sortField, order } = sortOptions(sort)
  const filterNodeId = nodeId === "all" ? null : Number(nodeId)

  useEffect(() => {
    const timer = window.setTimeout(() => {
      setDebouncedQuery(query.trim())
      setPage(1)
    }, 250)
    return () => window.clearTimeout(timer)
  }, [query])

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), REFRESH_MS)
    return () => window.clearInterval(timer)
  }, [])

  useEffect(() => {
    let current = true
    async function load() {
      if (busy.current) return
      busy.current = true
      if (!loaded.current) setLoading(true)
      try {
        const options = {
          q: debouncedQuery,
          node_id: filterNodeId,
          sort: sortField,
          order,
          page,
          page_size: PAGE_SIZE,
        } as const
        const overviewRequest = getProxyTrafficOverview()
        if (tab === "users") {
          const [nextOverview, rows] = await Promise.all([overviewRequest, listProxyTrafficUserRows(options)])
          if (!current) return
          setOverview(nextOverview)
          setUserPage(rows)
        } else {
          const [nextOverview, rows] = await Promise.all([overviewRequest, listProxyTrafficNodeRows(options)])
          if (!current) return
          setOverview(nextOverview)
          setNodePage(rows)
        }
        if (!current) return
        setError("")
        loaded.current = true
      } catch (cause) {
        if (current) setError((cause as Error).message || "读取流量统计失败")
      } finally {
        busy.current = false
        if (current) setLoading(false)
      }
    }
    void load()
    const timer = window.setInterval(() => void load(), REFRESH_MS)
    return () => {
      current = false
      window.clearInterval(timer)
    }
  }, [tab, debouncedQuery, filterNodeId, sortField, order, page, refreshKey])

  const visibleTotal = tab === "users" ? userPage?.total ?? 0 : nodePage?.total ?? 0
  const pageCount = Math.max(1, Math.ceil(visibleTotal / PAGE_SIZE))
  const onTab = (next: Tab) => {
    setTab(next)
    setPage(1)
  }

  return <div className="space-y-4">
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

    <div className="grid w-full grid-cols-2 rounded-lg bg-muted p-1 sm:w-fit" role="tablist" aria-label="流量统计类别">
      {([ ["users", "用户流量"], ["nodes", "代理节点流量"] ] as const).map(([value, label]) => (
        <button key={value} type="button" role="tab" aria-selected={tab === value} onClick={() => onTab(value)}
          className={`rounded-md px-4 py-2 text-sm font-medium transition-colors ${tab === value ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground"}`}>
          {label}
        </button>
      ))}
    </div>
    <p className="text-xs text-muted-foreground">顶部统计按用户业务流量汇总；代理节点页按服务器汇总启用代理 inbound 流量，两种视角不会相加。</p>

    <div className="flex flex-wrap items-center gap-2">
      <AdminSearchInput value={query} onChange={setQuery} placeholder={tab === "users" ? "搜索用户名或服务器" : "搜索服务器名称"} />
      <Select value={nodeId} onValueChange={(value) => { setNodeId(value); setPage(1) }}>
        <SelectTrigger className="w-40" aria-label="按服务器筛选"><SelectValue /></SelectTrigger>
        <SelectContent position="popper">
          <SelectItem value="all">全部服务器</SelectItem>
          {nodes.map((node) => <SelectItem key={node.id} value={String(node.id)}>{node.name}</SelectItem>)}
        </SelectContent>
      </Select>
      <Select value={sort} onValueChange={(value) => { setSort(value as Sort); setPage(1) }}>
        <SelectTrigger className="w-44" aria-label="流量排序"><SelectValue /></SelectTrigger>
        <SelectContent position="popper">
          <SelectItem value="total_desc">总流量从高到低</SelectItem>
          <SelectItem value="total_asc">总流量从低到高</SelectItem>
          <SelectItem value="uplink_desc">上传从高到低</SelectItem>
          <SelectItem value="uplink_asc">上传从低到高</SelectItem>
          <SelectItem value="downlink_desc">下载从高到低</SelectItem>
          <SelectItem value="downlink_asc">下载从低到高</SelectItem>
        </SelectContent>
      </Select>
    </div>

    {error && <div className="flex flex-wrap items-center gap-2 text-sm text-destructive" role="alert">
      <span>读取流量统计失败：{error}</span>
      <Button variant="outline" size="sm" onClick={() => setRefreshKey((value) => value + 1)}>重试</Button>
    </div>}

    <Card className="overflow-x-auto p-0">
      {tab === "users" ? <Table>
        <TableHeader className="bg-muted/50"><TableRow>
          <TableHead>用户</TableHead><TableHead>服务器</TableHead><TableHead className="text-right">上传流量</TableHead>
          <TableHead className="text-right">下载流量</TableHead><TableHead className="text-right">总流量</TableHead><TableHead>最近采集</TableHead>
        </TableRow></TableHeader>
        <TableBody>
          {userPage?.items.map((row) => <TableRow key={`${row.user_id}:${row.node_id}`} className="h-12">
            <TableCell className="font-medium">{row.username}</TableCell><TableCell>{row.node_name}</TableCell>
            <TableCell className="tnum text-right">{bytes(row.uplink_bytes)}</TableCell><TableCell className="tnum text-right">{bytes(row.downlink_bytes)}</TableCell>
            <TableCell className="tnum text-right font-medium">{bytes(row.total_bytes)}</TableCell>
            <TableCell className="text-sm text-muted-foreground" title={row.last_seen_at === null ? undefined : new Date(row.last_seen_at * 1000).toLocaleString()}>{relativeTime(row.last_seen_at, now)}</TableCell>
          </TableRow>)}
          {!loading && userPage?.items.length === 0 && <TableRow><TableCell colSpan={6} className="py-10 text-center text-sm text-muted-foreground">{debouncedQuery || nodeId !== "all" ? "没有匹配的流量记录" : "尚无用户流量记录"}</TableCell></TableRow>}
          {loading && !userPage && <TableRow><TableCell colSpan={6} className="p-4"><Skeleton className="h-10 w-full" /></TableCell></TableRow>}
        </TableBody>
      </Table> : <Table>
        <TableHeader className="bg-muted/50"><TableRow>
          <TableHead>代理服务器</TableHead><TableHead>协议</TableHead><TableHead className="text-right">上传流量</TableHead>
          <TableHead className="text-right">下载流量</TableHead><TableHead className="text-right">总流量</TableHead><TableHead>最近采集</TableHead>
        </TableRow></TableHeader>
        <TableBody>
          {nodePage?.items.map((row) => <TableRow key={row.node_id} className="h-12">
            <TableCell className="font-medium">{row.node_name}</TableCell>
            <TableCell>{row.protocols.length ? <div className="flex flex-wrap gap-1">{row.protocols.map((protocol) => <ProxyProtocolBadge key={protocol} protocol={protocol} />)}</div> : <Badge variant="outline">—</Badge>}</TableCell>
            <TableCell className="tnum text-right">{bytes(row.uplink_bytes)}</TableCell><TableCell className="tnum text-right">{bytes(row.downlink_bytes)}</TableCell>
            <TableCell className="tnum text-right font-medium">{bytes(row.total_bytes)}</TableCell>
            <TableCell className="text-sm text-muted-foreground" title={row.last_seen_at === null ? undefined : new Date(row.last_seen_at * 1000).toLocaleString()}>{relativeTime(row.last_seen_at, now)}</TableCell>
          </TableRow>)}
          {!loading && nodePage?.items.length === 0 && <TableRow><TableCell colSpan={6} className="py-10 text-center text-sm text-muted-foreground">{debouncedQuery || nodeId !== "all" ? "没有匹配的代理服务器" : "尚无代理节点流量记录"}</TableCell></TableRow>}
          {loading && !nodePage && <TableRow><TableCell colSpan={6} className="p-4"><Skeleton className="h-10 w-full" /></TableCell></TableRow>}
        </TableBody>
      </Table>}
      <CardContent className="flex flex-wrap items-center justify-between gap-2 border-t py-3 text-sm text-muted-foreground">
        <span>共 {visibleTotal} 条</span>
        <div className="flex items-center gap-2">
          <span>第 {Math.min(page, pageCount)} / {pageCount} 页</span>
          <Button variant="outline" size="sm" disabled={page <= 1 || loading} onClick={() => setPage((value) => Math.max(1, value - 1))}>上一页</Button>
          <Button variant="outline" size="sm" disabled={page >= pageCount || loading} onClick={() => setPage((value) => value + 1)}>下一页</Button>
        </div>
      </CardContent>
    </Card>
  </div>
}
