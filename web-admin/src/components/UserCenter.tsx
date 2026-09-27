import { useCallback, useEffect, useState } from "react"
import { Copy, LogOut, Moon, RefreshCw, Sun } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Skeleton } from "@/components/ui/skeleton"
import { ProxyAddressTypeBadge, ProxyProtocolBadge } from "@/components/ProxyBadges"
import { api } from "@/lib/api"

type UserProfile = {
  id: number
  username: string
  enabled: boolean
  expires_at: number | null
}

type PortalProxy = {
  id: number
  node_id: number
  node_name: string
  name: string
  protocol: string
  address: string
  address_type: string
  port: number
  server_name: string
  server_port: number
  online: boolean
}

type SubscriptionLinks = { clash: string; sing_box: string }

function displayDate(timestamp: number | null): string {
  if (timestamp === null) return "永久有效"
  const date = new Date(timestamp * 1000 - 1000)
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`
}

function absoluteLink(path: string): string {
  return new URL(path, location.origin).toString()
}

export function UserCenter({ siteName, dark, toggleTheme, signOut }: {
  siteName: string
  dark: boolean
  toggleTheme: () => void
  signOut: () => void
}) {
  const [profile, setProfile] = useState<UserProfile | null>(null)
  const [proxies, setProxies] = useState<PortalProxy[] | null>(null)
  const [links, setLinks] = useState<SubscriptionLinks | null>(null)
  const [error, setError] = useState("")
  const [rotating, setRotating] = useState(false)

  const load = useCallback(async () => {
    setError("")
    try {
      const [nextProfile, nextProxies, nextLinks] = await Promise.all([
        api<UserProfile>("/user/me"),
        api<{ items: PortalProxy[] }>("/user/me/proxies"),
        api<SubscriptionLinks>("/user/me/subscription"),
      ])
      setProfile(nextProfile)
      setProxies(nextProxies.items)
      setLinks(nextLinks)
    } catch (e) {
      setError((e as Error).message)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  async function copy(path: string) {
    try {
      await navigator.clipboard.writeText(absoluteLink(path))
      toast.success("订阅链接已复制")
    } catch {
      toast.error("复制失败，请检查浏览器剪贴板权限")
    }
  }

  async function rotateLinks() {
    if (!window.confirm("重置订阅链接后，旧链接会立即失效。继续吗？")) return
    setRotating(true)
    try {
      await api("/user/me/subscription-token", { method: "POST" })
      const next = await api<SubscriptionLinks>("/user/me/subscription")
      setLinks(next)
      toast.success("订阅链接已重置")
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setRotating(false)
    }
  }

  async function logout() {
    await api("/auth/logout", { method: "POST" }).catch(() => {})
    signOut()
  }

  return (
    <div className="min-h-svh">
      <header className="sticky top-0 z-10 border-b bg-background/80 backdrop-blur">
        <div className="mx-auto flex max-w-6xl items-center gap-3 px-4 py-3">
          <a href="/" className="font-semibold transition-opacity hover:opacity-70">{siteName || "Monitor"}</a>
          <span className="text-xs text-muted-foreground">用户中心</span>
          <div className="flex-1" />
          <Button variant="ghost" size="icon" onClick={toggleTheme} title="切换主题">
            {dark ? <Sun /> : <Moon />}
          </Button>
          <Button variant="ghost" size="icon" onClick={() => void logout()} title="退出登录">
            <LogOut />
          </Button>
        </div>
      </header>

      <main className="mx-auto max-w-6xl space-y-5 px-4 py-6">
        {error && <p role="alert" className="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">加载失败：{error}</p>}
        {!profile ? (
          <Skeleton className="h-32" />
        ) : (
          <Card className="gap-4 p-5 shadow-sm md:p-6">
            <div className="grid gap-5 sm:grid-cols-2 lg:grid-cols-4 lg:gap-6">
              <div className="min-w-0">
                <div className="text-xs text-muted-foreground">用户名</div>
                <h1 className="mt-1 truncate text-lg font-semibold">{profile.username}</h1>
                <Badge variant="outline" className={`mt-2 font-normal ${profile.enabled ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" : "border-destructive/40 bg-destructive/10 text-destructive"}`}>{profile.enabled ? "账号正常" : "已禁用"}</Badge>
              </div>
              <div>
                <div className="text-xs text-muted-foreground">流量使用</div>
                <div className="mt-1 text-sm font-medium">暂无流量数据</div>
              </div>
              <div>
                <div className="text-xs text-muted-foreground">流量重置</div>
                <div className="mt-1 text-sm font-medium">暂无重置信息</div>
              </div>
              <div>
                <div className="text-xs text-muted-foreground">到期时间</div>
                <div className="mt-1 text-sm font-medium">{displayDate(profile.expires_at)}</div>
              </div>
            </div>
          </Card>
        )}

        <Card className="gap-4 p-5 shadow-sm md:p-6">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h2 className="font-semibold">我的订阅</h2>
              <p className="mt-1 text-sm text-muted-foreground">订阅链接可导入对应客户端，请勿分享给他人。</p>
            </div>
            <Button variant="outline" size="sm" disabled={rotating || !links} onClick={() => void rotateLinks()}>
              <RefreshCw />重置订阅链接
            </Button>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            {([
              { key: "clash", title: "Clash / Mihomo", description: "适用于 Clash、Mihomo 客户端" },
              { key: "sing_box", title: "Sing-box", description: "适用于 Sing-box 客户端" },
            ] as const).map(({ key, title, description }) => {
              const path = links?.[key]
              const url = path ? absoluteLink(path) : ""
              return (
                <div key={key} className="min-w-0 rounded-lg border bg-background p-4">
                  <div className="font-medium">{title}</div>
                  <div className="mt-1 text-xs text-muted-foreground">{description}</div>
                  <div className="mt-3 flex min-w-0 items-center gap-2 rounded-md border bg-muted/30 px-3 py-2">
                    <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground" title={url}>{url || "加载中…"}</span>
                    <Button variant="outline" size="sm" className="shrink-0" disabled={!path} onClick={() => path && void copy(path)}><Copy />复制订阅</Button>
                  </div>
                </div>
              )
            })}
          </div>
        </Card>

        <Card className="gap-4 p-5 shadow-sm md:p-6">
          <div>
            <h2 className="font-semibold">我的节点</h2>
            <p className="mt-1 text-sm text-muted-foreground">这里只显示当前账号已启用授权的节点。</p>
          </div>
          {!proxies ? (
            <Skeleton className="h-24" />
          ) : proxies.length === 0 ? (
            <p className="rounded-md border border-dashed p-6 text-center text-sm text-muted-foreground">当前没有已启用的节点授权</p>
          ) : (
            <div className="overflow-x-auto rounded-md border">
              <table className="w-full min-w-[760px] table-fixed text-left text-sm">
                <thead className="bg-muted/50 text-xs text-muted-foreground">
                  <tr><th className="w-[22%] px-4 py-3">名称</th><th className="w-[19%] px-4 py-3">协议</th><th className="w-[25%] px-4 py-3">地址</th><th className="w-[9%] px-4 py-3">端口</th><th className="w-[17%] px-4 py-3">SNI</th><th className="w-[8%] px-4 py-3">状态</th></tr>
                </thead>
                <tbody>
                  {proxies.map((proxy) => (
                    <tr key={proxy.id} className="border-t transition-colors hover:bg-muted/30">
                      <td className="px-4 py-3"><div className="truncate font-medium" title={proxy.name}>{proxy.name}</div><div className="truncate text-xs text-muted-foreground" title={proxy.node_name}>{proxy.node_name}</div></td>
                      <td className="px-4 py-3"><ProxyProtocolBadge protocol={proxy.protocol} /></td>
                      <td className="px-4 py-3"><div className="flex min-w-0 items-center gap-2"><ProxyAddressTypeBadge addressType={proxy.address_type} /><span className="truncate font-mono text-xs" title={proxy.address}>{proxy.address}</span></div></td>
                      <td className="px-4 py-3 font-mono">{proxy.port}</td>
                      <td className="px-4 py-3"><span className="block truncate font-mono text-xs" title={proxy.server_name ? `${proxy.server_name}:${proxy.server_port}` : "—"}>{proxy.server_name ? `${proxy.server_name}:${proxy.server_port}` : "—"}</span></td>
                      <td className="px-4 py-3"><Badge variant="outline" className={`font-normal ${proxy.online ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" : "text-muted-foreground"}`}><span className={`size-1.5 rounded-full ${proxy.online ? "bg-emerald-500" : "bg-muted-foreground/50"}`} />{proxy.online ? "在线" : "离线"}</Badge></td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Card>
      </main>
    </div>
  )
}
