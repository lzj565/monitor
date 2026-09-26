import type { ProxyUser } from "./api.ts"

export const PROXY_USER_TABLE_COLUMNS = [
  { key: "id", label: "ID", className: "w-[70px]" },
  { key: "name", label: "用户名", className: "w-[200px]" },
  { key: "traffic", label: "流量使用情况", className: "w-[280px]" },
  { key: "status", label: "账户状态", className: "w-[150px]" },
  { key: "subscription", label: "专属订阅链接", className: "w-[260px]" },
  { key: "actions", label: "操作", className: "w-[180px] text-right" },
] as const

export type ProxyUserRowAction = "edit" | "delete"

export function proxyUserRowView(user: Pick<ProxyUser, "id" | "is_system">) {
  return {
    id: user.id,
    systemLabel: user.is_system ? "系统" : null,
    traffic: "--",
    trafficClearDisabled: true,
    trafficTooltip: "流量统计接入后可用",
    subscription: "尚未支持",
    subscriptionActionsDisabled: true,
    subscriptionTooltip: "用户专属订阅暂未支持",
    actions: (user.is_system ? ["edit"] : ["edit", "delete"]) as ProxyUserRowAction[],
  }
}

export function proxyUserCountText(userCount: number, loading: boolean): string {
  return loading ? "正在加载用户…" : `当前用户：${userCount} 户`
}

export function proxyUserListState(userCount: number, loading: boolean): "loading" | "empty" | "ready" {
  if (loading && userCount === 0) return "loading"
  return userCount === 0 ? "empty" : "ready"
}
