import { Copy, Layers, PanelsTopLeft, Smartphone } from "lucide-react"

import { Button } from "@/components/ui/button"

const CLIENTS = [
  { name: "Clash / Verge", description: "一键导入 Clash 配置", action: "导入", icon: PanelsTopLeft },
  { name: "小火箭 Shadowrocket", description: "iOS 专属导入", action: "导入", icon: Smartphone },
  { name: "Sing-box", description: "跨平台通用核心", action: "导入", icon: Layers },
  { name: "v2rayN / Nekobox", description: "适用于桌面与移动端", action: "复制订阅", icon: Copy },
] as const

export function ClientImportCard({ onUse }: { onUse: (clientName: string) => void }) {
  return (
    <section className="space-y-3 border-t pt-5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-medium">快捷一键导入到客户端</h3>
        <span className="text-xs text-muted-foreground">选择客户端后复制订阅地址</span>
      </div>
      <div className="grid gap-2.5 sm:grid-cols-2">
        {CLIENTS.map(({ name, description, action, icon: Icon }) => (
          <div key={name} className="flex min-w-0 items-center gap-3 rounded-lg border bg-card p-3 transition-colors hover:border-primary/25 hover:bg-muted/30">
            <div className="grid size-9 shrink-0 place-items-center rounded-md bg-muted text-muted-foreground">
              <Icon className="size-4" />
            </div>
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium">{name}</p>
              <p className="truncate text-xs text-muted-foreground">{description}</p>
            </div>
            <Button size="xs" variant="outline" onClick={() => onUse(name)}>{action}</Button>
          </div>
        ))}
      </div>
    </section>
  )
}
