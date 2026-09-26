import { useState } from "react"
import { Activity, Ban, CalendarDays, CircleCheck, CircleX, Database, Download, RefreshCw, TriangleAlert, Upload, Users } from "lucide-react"
import { toast } from "sonner"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Progress } from "@/components/ui/progress"
import { bytes } from "@/lib/format"
import type { UserCenterData, UserCenterStatus } from "@/lib/user-center-mock"

function UserStatusBadge({ status }: { status: UserCenterStatus }) {
  if (status === "active") {
    return <Badge variant="outline" className="gap-1.5 border-emerald-600/20 bg-emerald-600/5 text-emerald-700 dark:text-emerald-400"><CircleCheck />正常使用中</Badge>
  }
  if (status === "exhausted") {
    return <Badge variant="outline" className="gap-1.5 border-amber-600/25 bg-amber-500/10 text-amber-800 dark:text-amber-300"><TriangleAlert />流量已用尽</Badge>
  }
  if (status === "expired") {
    return <Badge variant="outline" className="gap-1.5 border-orange-600/20 bg-orange-500/10 text-orange-800 dark:text-orange-300"><CircleX />账号已过期</Badge>
  }
  return <Badge variant="outline" className="gap-1.5 border-destructive/20 bg-destructive/5 text-destructive"><Ban />账号已停用</Badge>
}

function expiryLabel(expireAt: string | null) {
  if (!expireAt) return "永不过期"
  const date = new Date(expireAt)
  if (!Number.isFinite(date.getTime())) return "—"
  return new Intl.DateTimeFormat("zh-CN", {
    timeZone: "Asia/Shanghai",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
  }).format(date)
}

function statusNotice(status: UserCenterStatus): string | null {
  if (status === "exhausted") return "本周期流量额度已用完，代理服务暂不可用。下次流量重置后将自动恢复。"
  if (status === "expired") return "当前账户已过期，代理服务暂不可用。"
  if (status === "disabled") return "当前账户已被停用，请联系管理员。"
  return null
}

export function TrafficQuotaCard({ data }: { data: UserCenterData }) {
  const [refreshing, setRefreshing] = useState(false)
  const { traffic, devices, user, expireAt } = data
  const unlimited = traffic.limitBytes <= 0
  const percent = unlimited ? 0 : Math.min(100, Math.max(0, traffic.usedBytes / traffic.limitBytes * 100))
  const remaining = unlimited ? 0 : Math.max(0, traffic.limitBytes - traffic.usedBytes)
  const notice = statusNotice(user.status)

  function refresh() {
    setRefreshing(true)
    window.setTimeout(() => {
      setRefreshing(false)
      toast.success("演示统计已刷新")
    }, 450)
  }

  const stats = [
    { label: "上传流量", value: bytes(traffic.uploadBytes), icon: Upload },
    { label: "下载流量", value: bytes(traffic.downloadBytes), icon: Download },
    { label: "剩余额度", value: unlimited ? "不限量" : bytes(remaining), icon: Database },
    { label: "在线设备", value: `${devices.online} / ${devices.limit > 0 ? devices.limit : "不限"}`, icon: Users },
    { label: "到期时间", value: expiryLabel(expireAt), icon: CalendarDays },
  ]

  return (
    <Card className="gap-0 overflow-hidden py-0">
      <div className="flex flex-wrap items-start justify-between gap-3 border-b px-5 py-4 sm:items-center sm:px-6">
        <div className="space-y-1">
          <h1 className="text-lg font-semibold tracking-tight">流量与订阅配额</h1>
          <p className="text-sm text-muted-foreground">实时统计所有代理节点产生的双向网络吞吐流量</p>
        </div>
        <div className="flex items-center gap-2">
          <UserStatusBadge status={user.status} />
          <Button size="sm" variant="outline" disabled={refreshing} onClick={refresh}>
            <RefreshCw className={refreshing ? "animate-spin" : ""} /> 刷新统计
          </Button>
        </div>
      </div>

      <div className="space-y-5 p-5 sm:p-6">
        {notice && (
          <div role="status" className="flex items-start gap-2 rounded-lg border border-amber-600/20 bg-amber-500/5 px-3.5 py-3 text-sm text-amber-900 dark:text-amber-200">
            <Activity className="mt-0.5 size-4 shrink-0" />
            <p>{notice}</p>
          </div>
        )}

        <div className="space-y-2.5">
          <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 text-sm">
            <p className="font-medium">已用：<span className="tnum">{bytes(traffic.usedBytes)}</span></p>
            <p className="text-muted-foreground">限额：<span className="font-medium text-foreground">{unlimited ? "不限量" : `${bytes(traffic.limitBytes)} (${Math.floor(percent)}%)`}</span></p>
          </div>
          {unlimited ? (
            <div role="img" aria-label="无限流量额度" className="h-2 rounded-full border border-dashed border-primary/25 bg-primary/5" />
          ) : (
            <Progress className="h-2" aria-label="流量使用比例" value={percent} indicatorClassName={user.status === "exhausted" ? "bg-amber-600" : "bg-primary"} />
          )}
          <div className="flex justify-between gap-4 text-xs text-muted-foreground">
            <span>起始：0 B</span>
            <span>剩余：{unlimited ? "不限量" : bytes(remaining)}</span>
          </div>
        </div>

        <div className="grid grid-cols-1 gap-3 min-[480px]:grid-cols-2 md:grid-cols-3 xl:grid-cols-5">
          {stats.map(({ label, value, icon: Icon }) => (
            <div key={label} className="min-w-0 rounded-lg border bg-muted/35 p-3.5 sm:p-4">
              <div className="flex items-center justify-between gap-2">
                <p className="text-xs text-muted-foreground">{label}</p>
                <Icon className="size-4 shrink-0 text-muted-foreground" />
              </div>
              <p className="mt-2 truncate text-base font-semibold tabular-nums" title={value}>{value}</p>
            </div>
          ))}
        </div>
      </div>
    </Card>
  )
}
