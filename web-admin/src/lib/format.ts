const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"]

const unitOf = (n: number) => Math.min(Math.floor(Math.log(n) / Math.log(1024)), UNITS.length - 1)

/**
 * 1024-based, as VPS dashboards and `df` report bytes, but labelled MB/GB the way
 * `df -h` and hosting plans write them. Three significant digits by default. Kept
 * in step with the theme's copy of this file.
 */
export function bytes(n: number, digits?: number): string {
  // `< 1` rather than `< 0`: a fraction of a byte puts `unitOf` at -1 and prints
  // "512 undefined".
  if (!n || n < 1) return "0 B"
  const i = unitOf(n)
  const v = n / 1024 ** i
  return `${v.toFixed(i === 0 ? 0 : (digits ?? (v >= 100 ? 0 : v >= 10 ? 1 : 2)))} ${UNITS[i]}`
}

export function uptime(seconds: number): string {
  if (!seconds) return "—"
  const d = Math.floor(seconds / 86400)
  const h = Math.floor((seconds % 86400) / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  return d > 0 ? `${d} 天 ${h} 小时` : h > 0 ? `${h} 小时 ${m} 分` : `${m} 分`
}

export function relativeTime(timestamp: number | null, now = Date.now()): string {
  if (timestamp === null) return "尚未采集"
  const seconds = Math.max(0, Math.floor(now / 1000 - timestamp))
  if (seconds < 5) return "刚刚"
  if (seconds < 60) return `${seconds} 秒前`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes} 分钟前`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时前`
  const days = Math.floor(hours / 24)
  if (days < 30) return `${days} 天前`
  return new Date(timestamp * 1000).toLocaleString()
}

/**
 * No expiry and no traffic cap are both rendered as the absence of a ceiling.
 * U+221E rather than the emoji, which arrives as a coloured tile from whatever
 * font the browser provides; this inherits the text colour and size.
 */
export const FOREVER = "∞"

const SYMBOLS: Record<string, string> = { USD: "$", CNY: "¥", EUR: "€", GBP: "£", JPY: "¥" }

export function money(amount: number, currency: string): string {
  return `${SYMBOLS[currency] ?? ""}${amount.toFixed(2)}${SYMBOLS[currency] ? "" : ` ${currency}`}`
}

// The hub stores these lengths under a name and any other as `<n>m`.
const NAMED_CYCLES: Record<string, number> = { monthly: 1, quarterly: 3, semiannual: 6, yearly: 12, biennial: 24, triennial: 36 }

/** A billing cycle in months: 0 for one-off, NaN when unrecognized. */
export function cycleMonths(cycle: string): number {
  return cycle === "once" ? 0 : NAMED_CYCLES[cycle] ?? Number(/^(\d+)m$/.exec(cycle)?.[1])
}
