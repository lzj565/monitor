export type TrafficStatus = { state?: string; reason?: string; fresh?: boolean; gaps?: number; discarded_records?: number; last_sample?: number; up_rate?: number; down_rate?: number }
export type TrafficRule = { key: string; node_id: number; rule_id: number; name: string; active: boolean; up: number; down: number; today: number; month: number; last_sample?: number; up_rate?: number | null; down_rate?: number | null }
export type Traffic = { hosts: { node_id: number; status: TrafficStatus }[]; rules: TrafficRule[] }
export function trafficBytes(value?: number | null): string {
  if (value == null || !Number.isFinite(value)) return "—"
  const units = ["B", "KiB", "MiB", "GiB", "TiB"]
  let i = 0
  while (value >= 1024 && i < units.length - 1) { value /= 1024; i++ }
  return `${value.toFixed(i ? 2 : 0)} ${units[i]}`
}
export function trafficState(s?: TrafficStatus): string {
  if (!s) return "等待 Agent 上报"
  if (s.state === "unmonitored") return `未监控：${s.reason || "nft 不可用"}`
  if (s.state === "monitoring") return s.fresh ? "监控中" : "数据已过期 / 等待上报"
  return "等待有效规则"
}
