import { trafficUsage } from "@/lib/traffic-usage"

export function TrafficUsage({ used, limit }: { used: number; limit?: number }) {
  const { usedLabel, quotaLabel, percentage, fill } = trafficUsage(used, limit)
  const description = `已用 ${usedLabel}，${quotaLabel}`
  return <div className="w-full min-w-60 space-y-3">
    <div className="tnum flex items-center justify-between gap-3 whitespace-nowrap text-sm leading-5">
      <span className="font-medium text-foreground">{usedLabel}</span>
      <span className="text-muted-foreground">{quotaLabel}</span>
    </div>
    <div
      className="h-2.5 w-full overflow-hidden rounded-full bg-muted"
      role={percentage === undefined ? "img" : "progressbar"}
      aria-label={description}
      aria-valuemin={percentage === undefined ? undefined : 0}
      aria-valuemax={percentage === undefined ? undefined : 100}
      aria-valuenow={percentage === undefined ? undefined : Math.min(100, percentage)}
      aria-valuetext={percentage === undefined ? undefined : description}
    >
      {fill > 0 && <div className="h-full rounded-full bg-primary" style={{ width: `${fill}%` }} />}
    </div>
  </div>
}
