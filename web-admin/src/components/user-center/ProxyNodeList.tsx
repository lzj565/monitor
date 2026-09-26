import { useState } from "react"
import { Activity, Eye, EyeOff, Globe, Server } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import type { UserCenterNode } from "@/lib/user-center-mock"

function maskedAddress(node: UserCenterNode) {
  if (node.ipVersion !== "IPv4") return "地址已隐藏"
  const parts = node.address.split(".")
  if (parts.length !== 4) return "地址已隐藏"
  return `${parts[0]}.***.***.${parts[3]}`
}

function ProxyNodeCard({ node }: { node: UserCenterNode }) {
  const [showAddress, setShowAddress] = useState(false)
  const address = showAddress ? node.address : maskedAddress(node)

  return (
    <div className="flex flex-col gap-4 rounded-xl border bg-card p-4 transition-colors hover:border-primary/25 hover:bg-muted/20 sm:flex-row sm:items-center sm:justify-between sm:p-5">
      <div className="flex min-w-0 items-start gap-3">
        <span className="grid size-10 shrink-0 place-items-center rounded-lg bg-muted text-xl" aria-hidden="true">{node.flag}</span>
        <div className="min-w-0 space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="font-medium">{node.name}</h3>
            <Badge variant="secondary" className="font-normal"><Globe />{node.region}</Badge>
            <Badge variant="outline" className="font-normal">{node.ipVersion}</Badge>
            <Badge variant="outline" className="font-normal">{node.protocol}</Badge>
          </div>
          <p className="flex flex-wrap items-center gap-x-2 gap-y-1 font-mono text-sm text-muted-foreground">
            <span>{node.region} · {address}:{node.port}</span>
            <span className="font-sans text-xs">{showAddress ? "公网地址" : "IP 已脱敏"}</span>
          </p>
        </div>
      </div>

      <div className="flex items-center justify-between gap-2 sm:justify-end">
        <Badge variant="outline" className={node.online ? "border-emerald-600/20 bg-emerald-600/5 text-emerald-700 dark:text-emerald-400" : "text-muted-foreground"}>
          <span className={`size-1.5 rounded-full ${node.online ? "bg-emerald-600" : "bg-muted-foreground"}`} />
          {node.online ? "在线" : "离线"}
        </Badge>
        <Button
          size="icon-sm"
          variant="ghost"
          title={showAddress ? "隐藏服务器 IP" : "查看服务器 IP"}
          aria-label={showAddress ? "隐藏服务器 IP" : "查看服务器 IP"}
          aria-pressed={showAddress}
          onClick={() => setShowAddress((visible) => !visible)}
        >
          {showAddress ? <EyeOff /> : <Eye />}
        </Button>
      </div>
    </div>
  )
}

export function ProxyNodeList({ nodes }: { nodes: UserCenterNode[] }) {
  return (
    <Card className="gap-0 overflow-hidden py-0">
      <div className="flex flex-wrap items-start justify-between gap-3 border-b px-5 py-4 sm:items-center sm:px-6">
        <div className="space-y-1">
          <h2 className="flex items-center gap-2 text-lg font-semibold tracking-tight"><Globe className="size-5 text-muted-foreground" />可用代理节点列表 ({nodes.length})</h2>
          <p className="text-sm text-muted-foreground">当前包含在您订阅中的节点，节点真实 IP 默认脱敏显示，可点击眼睛查看。</p>
        </div>
        <Button size="sm" variant="outline" asChild>
          <a href="/" target="_blank" rel="noreferrer"><Activity />服务器探针</a>
        </Button>
      </div>
      <div className="space-y-3 p-5 sm:p-6">
        {nodes.length > 0 ? nodes.map((node) => <ProxyNodeCard key={node.id} node={node} />) : (
          <div className="rounded-lg border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
            <Server className="mx-auto mb-2 size-5" />当前没有可用代理节点。
          </div>
        )}
      </div>
    </Card>
  )
}
