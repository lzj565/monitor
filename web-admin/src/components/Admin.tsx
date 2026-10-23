import { Activity, ArrowUpCircle, Bell, Database, KeyRound, Network, Palette, Radio, Server, Settings, Shield, Users } from "lucide-react"
import { behind, outdatedAgents, type Node } from "@/lib/api"
import { useVersions } from "./admin/hooks"
import { lazy, Suspense } from "react"

const Nodes = lazy(() => import("./admin/Nodes").then((m) => ({ default: m.Nodes })))
const Ping = lazy(() => import("./admin/Ping").then((m) => ({ default: m.Ping })))
const Themes = lazy(() => import("./admin/Themes").then((m) => ({ default: m.Themes })))
const SettingsTab = lazy(() => import("./admin/Settings").then((m) => ({ default: m.SettingsTab })))
const Notify = lazy(() => import("./admin/Settings").then((m) => ({ default: m.Notify })))
const Security = lazy(() => import("./admin/Security").then((m) => ({ default: m.Security })))
const Data = lazy(() => import("./admin/Data").then((m) => ({ default: m.Data })))
const Update = lazy(() => import("./admin/Update").then((m) => ({ default: m.Update })))
const ProxiesPage = lazy(() => import("@/components/Resources").then((m) => ({ default: m.ProxiesPage })))
const SubscriptionsResourcePage = lazy(() => import("@/components/Resources").then((m) => ({ default: m.SubscriptionsResourcePage })))
const UsersResourcePage = lazy(() => import("@/components/Resources").then((m) => ({ default: m.UsersResourcePage })))
const ProxyTrafficPage = lazy(() => import("@/components/ProxyTrafficPage").then((m) => ({ default: m.ProxyTrafficPage })))

// Each area is its own route rather than a tab, so a page can be linked to and a
// reload returns to the same section.
export const ADMIN_SECTIONS = [
  { path: "/admin/nodes", label: "节点", icon: Server },
  { path: "/admin/proxies", label: "代理", icon: Network },
  { path: "/admin/users", label: "用户", icon: Users },
  { path: "/admin/subscriptions", label: "订阅", icon: KeyRound },
  { path: "/admin/traffic", label: "流量", icon: Activity },
  { path: "/admin/ping", label: "延迟", icon: Radio },
  { path: "/admin/notify", label: "通知", icon: Bell },
  { path: "/admin/data", label: "数据", icon: Database },
  { path: "/admin/themes", label: "主题", icon: Palette },
  { path: "/admin/security", label: "安全", icon: Shield },
  { path: "/admin/settings", label: "设置", icon: Settings },
  { path: "/admin/update", label: "更新", icon: ArrowUpCircle },
] as const

export function Admin({
  path,
  search,
  go,
  nodes,
  refresh,
  site,
  refusal,
  reloadMe,
}: {
  path: string
  search: string
  go: (to: string) => void
  nodes: Node[]
  refresh: () => void
  site: string
  refusal: string
  reloadMe: () => void
}) {
  const { versions, reload } = useVersions()
  // One dot for both; the page separates them. Absent when switched off on that
  // page, or when the lookup failed and the page has nothing to say.
  const updates =
    !!versions?.notice
    && (behind(versions.hub, versions.hub_latest) || outdatedAgents(nodes, versions.agent_latest).length > 0)
  return (
    <div className="flex flex-col gap-6 md:flex-row">
      <nav className="flex gap-1 overflow-x-auto md:w-44 md:shrink-0 md:flex-col md:overflow-visible">
        {ADMIN_SECTIONS.map(({ path: to, label, icon: Icon }) => {
          const active = path === to
          return (
            <button
              key={to}
              onClick={() => go(to)}
              aria-current={active ? "page" : undefined}
              className={`flex shrink-0 items-center gap-2 rounded-md px-3 py-2 text-sm transition-colors ${
                active ? "bg-secondary font-medium" : "text-muted-foreground hover:bg-muted"
              }`}
            >
              <Icon className="size-4" />
              {label}
              {to === "/admin/update" && updates && (
                <span className="ml-1 size-1.5 shrink-0 rounded-full bg-foreground md:ml-auto" title="有新版本" />
              )}
            </button>
          )
        })}
      </nav>

      <div className="min-w-0 flex-1">
        <Suspense fallback={<p className="p-6 text-sm text-muted-foreground">加载中…</p>}>
        {path === "/admin/traffic" ? (
          <ProxyTrafficPage nodes={nodes} />
        ) : path === "/admin/proxies" ? (
          <ProxiesPage nodes={nodes} />
        ) : path === "/admin/users" ? (
          <UsersResourcePage />
        ) : path === "/admin/subscriptions" ? (
          <SubscriptionsResourcePage nodes={nodes} search={search} go={go} />
        ) : path === "/admin/ping" ? (
          <Ping nodes={nodes} />
        ) : path === "/admin/notify" ? (
          <Notify nodes={nodes} refresh={refresh} />
        ) : path === "/admin/data" ? (
          <Data />
        ) : path === "/admin/themes" ? (
          <Themes />
        ) : path === "/admin/security" ? (
          <Security site={site} />
        ) : path === "/admin/settings" ? (
          <SettingsTab onSaved={reloadMe} />
        ) : path === "/admin/update" ? (
          <Update versions={versions} reload={reload} nodes={nodes} site={site} refusal={refusal} />
        ) : (
          <Nodes nodes={nodes} refresh={refresh} site={site} refusal={refusal} />
        )}
        </Suspense>
      </div>
    </div>
  )
}
