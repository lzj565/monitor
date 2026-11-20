import type { Node } from "@/lib/api"
import { displayProxyName } from "@/lib/proxy-name"

export function ProxyNamePrefix({ name, node, includeNodeName, setIncludeNodeName }: {
  name: string
  node: Node | undefined
  includeNodeName: boolean
  setIncludeNodeName: (value: boolean) => void
}) {
  const displayName = displayProxyName({ name, include_node_name: includeNodeName, node_id: node?.id ?? 0 }, node)
  return (
    <div className="flex flex-wrap items-start gap-x-3 gap-y-1 text-sm">
      <label className="inline-flex shrink-0 cursor-pointer items-center gap-2">
        <input type="checkbox" className="size-4 accent-primary" checked={includeNodeName} onChange={(event) => setIncludeNodeName(event.target.checked)} />
        附加节点地区前缀
      </label>
      <span className="text-muted-foreground">将自动生成：{name.trim() ? displayName : "代理名称"}</span>
    </div>
  )
}
