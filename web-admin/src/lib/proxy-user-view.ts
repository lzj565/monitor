import type { ProxyUser } from "./api.ts"

export const PROXY_USER_TABLE_COLUMNS = [
  { key: "id", label: "ID", className: "w-[70px]" },
  { key: "name", label: "用户名", className: "w-[200px]" },
  { key: "traffic", label: "流量使用情况", className: "w-[320px]" },
  { key: "expiry", label: "到期时间", className: "w-[150px]" },
  { key: "status", label: "账户状态", className: "w-[180px]" },
  { key: "actions", label: "操作", className: "w-[160px] text-right" },
] as const

export type ProxyUserRowAction = "edit" | "delete"

export function proxyUserRowView(user: Pick<ProxyUser, "id" | "is_system">) {
  return {
    id: user.id,
    systemLabel: user.is_system ? "系统" : null,
    actions: (user.is_system ? ["edit"] : ["edit", "delete"]) as ProxyUserRowAction[],
  }
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B"
  const units = ["B", "KB", "MB", "GB", "TB", "PB"]
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  const value = bytes / 1024 ** exponent
  const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2
  return `${value.toFixed(digits).replace(/(\.\d*?[1-9])0+$|\.0+$/, "$1")} ${units[exponent]}`
}

export function trafficPercent(used: number, limit: number): number {
  if (limit <= 0) return 0
  return Math.min(100, Math.max(0, (used / limit) * 100))
}

export function expiryDateLabel(expireDate: string | null): string {
  if (!expireDate) return "永久有效"
  return expireDate.replaceAll("-", "/")
}

export function limitBytesFromGb(value: string): number | null {
  if (value.trim() === "") return null
  const gb = Number(value)
  const bytes = gb * 1024 ** 3
  if (!Number.isFinite(gb) || gb < 0 || !Number.isSafeInteger(Math.round(bytes))) return null
  return Math.round(bytes)
}

export function limitGbInput(bytes: number): string {
  if (bytes <= 0) return "0"
  return (bytes / 1024 ** 3).toFixed(9).replace(/(\.\d*?[1-9])0+$|\.0+$/, "$1")
}

export function proxyUserCountText(userCount: number, loading: boolean): string {
  return loading ? "正在加载用户…" : `当前用户：${userCount} 户`
}

export function proxyUserListState(userCount: number, loading: boolean): "loading" | "empty" | "ready" {
  if (loading && userCount === 0) return "loading"
  return userCount === 0 ? "empty" : "ready"
}
