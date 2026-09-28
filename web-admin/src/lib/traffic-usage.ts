const UNITS = ["B", "KB", "MB", "GB", "TB"]

export function formatTrafficBytes(value: number): string {
  const bytes = Number.isFinite(value) ? Math.max(0, value) : 0
  const unit = bytes < 1 ? 0 : Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), UNITS.length - 1)
  return `${(bytes / 1024 ** unit).toFixed(unit === 0 ? 0 : 2)} ${UNITS[unit]}`
}

export function trafficUsage(used: number, limit?: number) {
  const usedLabel = formatTrafficBytes(used)
  if (limit === undefined || !Number.isFinite(limit) || limit < 0) {
    return { usedLabel, quotaLabel: "额度未知", percentage: undefined, fill: 0 }
  }
  if (limit === 0) return { usedLabel, quotaLabel: "不限量", percentage: undefined, fill: 0 }
  const ratio = Math.max(0, Number.isFinite(used) ? used : 0) / limit * 100
  const percentage = Math.round(ratio)
  return {
    usedLabel,
    quotaLabel: `${formatTrafficBytes(limit)} (${percentage}%)`,
    percentage,
    fill: percentage === 0 ? 0 : Math.min(100, ratio),
  }
}
