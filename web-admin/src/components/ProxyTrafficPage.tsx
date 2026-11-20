import { Fragment, useEffect, useRef, useState } from "react"
import { Activity, Download, RefreshCw, Upload, Users } from "lucide-react"
import { toast } from "sonner"

import { AdminConfirmDialog } from "@/components/AdminShared"
import { ProxyProtocolBadge } from "@/components/ProxyBadges"
import { TrafficUsage } from "@/components/TrafficUsage"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { bytes } from "@/lib/format"
import {
  getProxyTrafficOverview, getProxyUserTraffic, listProxyTrafficNodeRows,
  listProxyTrafficUserSummaryRows, resetProxyNodeTraffic, resetProxyUserTraffic,
  type ProxyTrafficOverview, type ProxyTrafficSummary,
} from "@/lib/resources"

const PAGE_SIZE = 25
const REFRESH_MS = 5_000
type View = "user" | "node"
type ResetTarget = { kind: View; id: number; name: string }
type TrafficList = Awaited<ReturnType<typeof listProxyTrafficNodeRows | typeof listProxyTrafficUserSummaryRows>>

function StatCard({ label, value, icon: Icon }: { label: string; value: string; icon: typeof Upload }) {
  return <Card className="gap-2 p-4">
    <div className="flex items-center gap-2 text-sm text-muted-foreground"><Icon className="size-4" />{label}</div>
    <div className="tnum text-xl font-semibold tracking-tight">{value}</div>
  </Card>
}

function ByteCells({ row }: { row: { uplink_bytes: number; downlink_bytes: number; total_bytes: number } }) {
  return <>
    <TableCell className="tnum text-right">{bytes(row.uplink_bytes)}</TableCell>
    <TableCell className="tnum text-right">{bytes(row.downlink_bytes)}</TableCell>
    <TableCell className="tnum text-right">{bytes(row.total_bytes)}</TableCell>
  </>
}

function UserNodeDetails({ id }: { id: number }) {
  const [rows, setRows] = useState<ProxyTrafficSummary[] | null>(null)
  const [error, setError] = useState(false)
  const [retry, setRetry] = useState(0)

  useEffect(() => {
    let current = true
    getProxyUserTraffic(id).then((items) => {
      if (current) setRows(items)
    }).catch(() => {
      if (current) setError(true)
    })
    return () => { current = false }
  }, [id, retry])

  if (error) return <Button variant="outline" size="sm" onClick={() => {
    setError(false)
    setRetry((value) => value + 1)
  }}>重试加载节点明细</Button>
  if (!rows) return <span role="status">正在加载节点明细…</span>
  if (!rows.length) return <span>暂无节点用量记录</span>
  return <Table>
    <TableHeader><TableRow>
      <TableHead>节点</TableHead>
      <TableHead className="text-right">上传</TableHead>
      <TableHead className="text-right">下载</TableHead>
      <TableHead className="text-right">合计</TableHead>
    </TableRow></TableHeader>
    <TableBody>{rows.map((row) => <TableRow key={row.node_id}>
      <TableCell>{row.node_name}</TableCell>
      <ByteCells row={{ ...row, total_bytes: row.uplink_bytes + row.downlink_bytes }} />
    </TableRow>)}</TableBody>
  </Table>
}

export function ProxyTrafficPage() {
  const [view, setView] = useState<View>("user")
  const [search, setSearch] = useState("")
  const [query, setQuery] = useState("")
  const [page, setPage] = useState(1)
  const [overview, setOverview] = useState<ProxyTrafficOverview | null>(null)
  const [result, setResult] = useState<{ key: string; data: TrafficList } | null>(null)
  const [error, setError] = useState("")
  const [loading, setLoading] = useState(false)
  const [revision, setRevision] = useState(0)
  const [expanded, setExpanded] = useState<number | null>(null)
  const [resetRevision, setResetRevision] = useState(0)
  const [resetTarget, setResetTarget] = useState<ResetTarget | null>(null)
  const [resetting, setResetting] = useState(false)
  const resettingRef = useRef(false)
  const requestKey = JSON.stringify([view, query, page])
  const currentList = result?.key === requestKey ? result.data : null
  const columns = view === "user" ? 7 : 6

  useEffect(() => {
    const timer = window.setTimeout(() => {
      if (search.trim() !== query) {
        setQuery(search.trim())
        setPage(1)
        setExpanded(null)
      }
    }, 300)
    return () => window.clearTimeout(timer)
  }, [search, query])

  useEffect(() => {
    let current = true
    let busy = false
    async function load() {
      if (busy || document.hidden) return
      busy = true
      setLoading(true)
      const options = { q: query, node_id: null, sort: "total", order: "desc", page, page_size: PAGE_SIZE } as const
      const [summary, list] = await Promise.allSettled([
        getProxyTrafficOverview(),
        view === "user" ? listProxyTrafficUserSummaryRows(options) : listProxyTrafficNodeRows(options),
      ])
      busy = false
      if (!current) return
      setLoading(false)
      if (summary.status === "fulfilled") setOverview(summary.value)
      if (list.status === "fulfilled") {
        const lastPage = Math.max(1, Math.ceil(list.value.total / PAGE_SIZE))
        if (page > lastPage) setPage(lastPage)
        else setResult({ key: requestKey, data: list.value })
      }
      setError(summary.status === "rejected" || list.status === "rejected" ? "读取流量统计失败" : "")
    }
    void load()
    const interval = window.setInterval(() => void load(), REFRESH_MS)
    const onVisibilityChange = () => { if (!document.hidden) void load() }
    document.addEventListener("visibilitychange", onVisibilityChange)
    return () => {
      current = false
      window.clearInterval(interval)
      document.removeEventListener("visibilitychange", onVisibilityChange)
    }
  }, [view, query, page, requestKey, revision])

  function changeView(next: View) {
    if (view === next) return
    setView(next)
    setSearch("")
    setQuery("")
    setPage(1)
    setExpanded(null)
  }

  async function resetTraffic() {
    if (!resetTarget || resettingRef.current) return
    resettingRef.current = true
    setResetting(true)
    try {
      if (resetTarget.kind === "user") {
        await resetProxyUserTraffic(resetTarget.id)
        if (expanded === resetTarget.id) setResetRevision((value) => value + 1)
      } else {
        await resetProxyNodeTraffic(resetTarget.id)
      }
      setResetTarget(null)
      setRevision((value) => value + 1)
      toast.success("流量已重置")
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : "重置流量失败")
    } finally {
      resettingRef.current = false
      setResetting(false)
    }
  }

  return <div className="space-y-5">
    <div className="flex items-center justify-between gap-3">
      <h1 className="text-lg font-semibold">流量统计</h1>
      <Button variant="outline" size="sm" onClick={() => setRevision((value) => value + 1)} disabled={loading}>
        <RefreshCw className={loading ? "animate-spin" : undefined} /> 刷新
      </Button>
    </div>

    {overview && <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">
      <StatCard label="用户累计代理流量" value={bytes(overview.total_bytes)} icon={Activity} />
      <StatCard label="累计上传" value={bytes(overview.uplink_bytes)} icon={Upload} />
      <StatCard label="累计下载" value={bytes(overview.downlink_bytes)} icon={Download} />
      <StatCard label="近 60 秒采集用户" value={String(overview.active_users)} icon={Users} />
    </div>}

    <div className="flex flex-wrap items-center gap-3">
      <div role="group" aria-label="统计维度" className="flex gap-2">
        <Button size="sm" variant={view === "user" ? "default" : "outline"} aria-pressed={view === "user"} onClick={() => changeView("user")}>按用户</Button>
        <Button size="sm" variant={view === "node" ? "default" : "outline"} aria-pressed={view === "node"} onClick={() => changeView("node")}>按节点</Button>
      </div>
      <Input className="w-full sm:w-60" aria-label={view === "user" ? "搜索用户名" : "搜索节点名"}
        placeholder={view === "user" ? "搜索用户名" : "搜索节点名"} value={search}
        onChange={(event) => setSearch(event.target.value)} />
    </div>
    {error && <div role="alert" className="flex items-center gap-2 text-sm text-destructive">
      {error}
      <Button variant="outline" size="sm" onClick={() => setRevision((value) => value + 1)}>重试</Button>
    </div>}
    <Card className="overflow-hidden p-0">
      <Table className={view === "user" ? "min-w-[800px]" : "min-w-[680px]"} aria-label={view === "user" ? "用户流量" : "节点代理流量"}>
        <TableHeader><TableRow>
          <TableHead>{view === "user" ? "用户" : "节点及代理协议"}</TableHead>
          <TableHead className="text-right">上传</TableHead>
          <TableHead className="text-right">下载</TableHead>
          <TableHead className="text-right">合计</TableHead>
          {view === "user" && <TableHead>额度</TableHead>}
          <TableHead>{view === "user" ? "下次重置日" : "最近采集"}</TableHead>
          <TableHead className="text-right">操作</TableHead>
        </TableRow></TableHeader>
        <TableBody>
          {currentList?.items.map((row) => {
            const isUser = "user_id" in row
            const id = isUser ? row.user_id : row.node_id
            const name = isUser ? row.username : row.node_name
            return <Fragment key={id}>
              <TableRow>
                <TableCell className="whitespace-normal">
                  {isUser ? <Button variant="link" size="sm" className="h-auto max-w-full whitespace-normal p-0 text-left [overflow-wrap:anywhere]"
                    aria-expanded={expanded === id} onClick={() => setExpanded(expanded === id ? null : id)}>
                    {name}
                  </Button> : <div className="flex flex-wrap items-center gap-2">
                    <span className="[overflow-wrap:anywhere]">{name}</span>
                    {row.protocols.map((protocol) => <ProxyProtocolBadge key={protocol} protocol={protocol} />)}
                  </div>}
                </TableCell>
                <ByteCells row={row} />
                {isUser && <TableCell>{row.traffic_limit > 0
                  ? <TrafficUsage used={row.total_bytes} limit={row.traffic_limit} /> : "不限额"}</TableCell>}
                <TableCell>{isUser ? row.next_reset_date ?? "未设置"
                  : row.last_seen_at == null ? "尚未采集" : new Date(row.last_seen_at * 1000).toLocaleString()}</TableCell>
                <TableCell className="text-right">
                  <Button variant="ghost" size="sm" disabled={resetting} onClick={() => setResetTarget({ kind: view, id, name })}>重置流量</Button>
                </TableCell>
              </TableRow>
              {isUser && expanded === id && <TableRow>
                <TableCell colSpan={columns} className="bg-muted/40 p-4">
                  <UserNodeDetails key={resetRevision} id={id} />
                </TableCell>
              </TableRow>}
            </Fragment>
          })}
          {!currentList?.items.length && <TableRow><TableCell colSpan={columns} className="py-8 text-center text-muted-foreground">
            {currentList ? "暂无匹配记录" : error ? "暂时无法读取流量记录" : "正在加载流量记录…"}
          </TableCell></TableRow>}
        </TableBody>
      </Table>
    </Card>
    {currentList && <div className="flex flex-wrap items-center justify-end gap-2 text-sm">
      <span className="mr-auto text-muted-foreground">共 {currentList.total} 条 · 第 {page} 页</span>
      <Button size="sm" variant="outline" disabled={page <= 1} onClick={() => { setExpanded(null); setPage(page - 1) }}>上一页</Button>
      <Button size="sm" variant="outline" disabled={page * PAGE_SIZE >= currentList.total} onClick={() => { setExpanded(null); setPage(page + 1) }}>下一页</Button>
    </div>}
    {resetTarget && <AdminConfirmDialog
      title={`重置「${resetTarget.name}」的流量？`}
      description={resetTarget.kind === "user" ? "将清零该用户在所有节点的累计业务流量。" : "将清零该节点的累计代理业务流量。"}
      confirmLabel="重置流量" busy={resetting}
      onClose={() => { if (!resettingRef.current) setResetTarget(null) }}
      onConfirm={() => void resetTraffic()}
    />}
  </div>
}
