import { useCallback, useEffect, useState } from "react"
import { ArrowDown, ArrowUp, CalendarDays, Copy, FileCode, Gauge, Link, LogOut, Moon, RefreshCw, Shield, Sun } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Badge } from "@/components/ui/badge"
import { Skeleton } from "@/components/ui/skeleton"
import { ProxyAddressTypeBadge, ProxyProtocolBadge, TooltipText } from "@/components/ProxyBadges"
import { DragHandle, useDragOrder } from "@/components/DragOrder"
import { bytes } from "@/lib/format"
import { api } from "@/lib/api"

type UserProfile = {
  id: number
  username: string
  enabled: boolean
  expires_at: number | null
  traffic_limit: number
  traffic_reset_day: number
  traffic: { uplink_bytes: number; downlink_bytes: number; next_reset_date: string | null; reset_days_remaining: number | null }
}

type PortalProxy = {
  id: number
  node_id: number
  node_name: string
  node_group: string
  node_country: string
  name: string
  protocol: string
  address: string
  address_type: string
  port: number
  server_name: string
  server_port: number
  vless_url: string
  mihomo_yaml: string
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

export function UserCenter({ siteName, dark, toggleTheme, signOut, returnToAdmin }: {
  siteName: string
  dark: boolean
  toggleTheme: () => void
  signOut: () => void
  returnToAdmin?: () => void
}) {
  const [profile, setProfile] = useState<UserProfile | null>(null)
  const [proxies, setProxies] = useState<PortalProxy[] | null>(null)
  const [links, setLinks] = useState<SubscriptionLinks | null>(null)
  const [error, setError] = useState("")
  const [rotating, setRotating] = useState(false)
  const [loadAttempt, setLoadAttempt] = useState(0)

  const reloadProxies = useCallback(() => {
    void api<{ items: PortalProxy[] }>("/user/me/proxies").then((result) => setProxies(result.items)).catch((e: Error) => toast.error(e.message))
  }, [])
  const drag = useDragOrder(proxies ?? [], "user/me/proxies", reloadProxies)
  const used = profile ? profile.traffic.uplink_bytes + profile.traffic.downlink_bytes : 0
  const limit = profile?.traffic_limit ?? 0
  const percentage = limit > 0 ? used / limit * 100 : 0
  const exceeded = limit > 0 && used > limit

  useEffect(() => {
    let active = true
    async function load() {
      setError("")
      try {
        const [nextProfile, nextProxies, nextLinks] = await Promise.all([
          api<UserProfile>("/user/me"),
          api<{ items: PortalProxy[] }>("/user/me/proxies"),
          api<SubscriptionLinks>("/user/me/subscription"),
        ])
        if (!active) return
        setError("")
        setProfile(nextProfile)
        setProxies(nextProxies.items)
        setLinks(nextLinks)
      } catch (e) {
        if (active) setError((e as Error).message)
      }
    }
    void load()
    return () => { active = false }
  }, [loadAttempt])

  async function copy(text: string, label: string) {
    try {
      await navigator.clipboard.writeText(text)
      toast.success(`${label}已复制`)
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
    <div className="min-h-svh bg-muted/25">
      <header className="sticky top-0 z-10 border-b bg-background/80 backdrop-blur">
        <div className="mx-auto flex max-w-7xl items-center gap-3 px-4 py-3">
          <a href="/" className="font-semibold transition-opacity hover:opacity-70">{siteName || "Monitor"}</a>
          <span className="text-xs text-muted-foreground">用户中心</span>
          <div className="flex-1" />
          {returnToAdmin && <Button variant="ghost" size="sm" onClick={returnToAdmin}><Shield /> 返回后台</Button>}
          <Button variant="ghost" size="icon" onClick={toggleTheme} title="切换主题">
            {dark ? <Sun /> : <Moon />}
          </Button>
          <Button variant="ghost" size="icon" onClick={() => void logout()} title="退出登录">
            <LogOut />
          </Button>
        </div>
      </header>

      <main className="mx-auto max-w-7xl space-y-6 px-4 py-6 sm:px-6 sm:py-8">
        {error && <div role="alert" className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-destructive/20 bg-destructive/10 px-4 py-3 text-sm text-destructive"><span>加载失败：{error}</span><Button variant="outline" size="sm" onClick={() => setLoadAttempt((attempt) => attempt + 1)}>重新加载</Button></div>}
        {!profile ? (
          error ? null : <Skeleton className="h-40 rounded-xl" />
        ) : (
          <section className="space-y-5" aria-label="账号与流量概览">
            <div className="flex flex-wrap items-center justify-between gap-4">
              <div className="min-w-0">
                <p className="mb-1 text-xs font-medium tracking-wider text-muted-foreground">账户概览</p>
                <h1 className="break-all text-2xl font-semibold tracking-tight">{profile.username}</h1>
                <p className="mt-1 text-sm text-muted-foreground">查看用量、管理订阅与节点</p>
              </div>
              <Badge variant="outline" className={`gap-2 rounded-full px-3 py-1.5 ${profile.enabled ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" : "border-destructive/30 bg-destructive/10 text-destructive"}`}>
                <span className={`size-1.5 rounded-full ${profile.enabled ? "bg-emerald-500" : "bg-destructive"}`} />{profile.enabled ? "账号正常" : "已禁用"}
              </Badge>
            </div>
            <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
              <Card className="gap-4 p-5 shadow-sm md:col-span-2 sm:p-6">
                <div className="flex items-center justify-between text-sm"><span className="flex items-center gap-2 text-muted-foreground"><Gauge className="size-4" />流量使用</span><span className={exceeded ? "text-destructive" : "text-muted-foreground"}>{limit > 0 ? `${percentage.toFixed(1)}% 已用` : "不限流量"}</span></div>
                <div className="flex flex-wrap items-baseline gap-2"><span className="text-3xl font-semibold tabular-nums tracking-tight">{bytes(used)}</span><span className="text-sm text-muted-foreground">{limit > 0 ? `/ ${bytes(limit)}` : "累计使用"}</span></div>
                <div className="h-2 overflow-hidden rounded-full bg-muted" role={limit > 0 ? "progressbar" : "img"} aria-label="流量使用" aria-valuemin={limit > 0 ? 0 : undefined} aria-valuemax={limit > 0 ? 100 : undefined} aria-valuenow={limit > 0 ? Math.min(100, percentage) : undefined} aria-valuetext={`${bytes(used)} / ${limit > 0 ? bytes(limit) : "不限流量"}`}>
                  <div className={`h-full rounded-full transition-all ${exceeded ? "bg-destructive" : "bg-sky-500"}`} style={{ width: `${Math.min(100, percentage)}%` }} />
                </div>
                <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
                  <span className="flex items-center gap-3"><span className="flex items-center gap-1"><ArrowUp className="size-3" />{bytes(profile.traffic.uplink_bytes)}</span><span className="flex items-center gap-1"><ArrowDown className="size-3" />{bytes(profile.traffic.downlink_bytes)}</span></span>
                  {limit > 0 && <span className={exceeded ? "text-destructive" : ""}>{exceeded ? `已超出 ${bytes(used - limit)}` : `剩余 ${bytes(limit - used)}`}</span>}
                </div>
              </Card>
              <Card className="justify-between gap-4 p-5 shadow-sm sm:p-6">
                <div className="flex items-center gap-2 text-sm text-muted-foreground"><RefreshCw className="size-4" />流量重置</div>
                <div><div className="text-2xl font-semibold tracking-tight">{profile.traffic_reset_day ? `每月 ${profile.traffic_reset_day} 日` : "不自动重置"}</div><p className="mt-2 text-xs text-muted-foreground">{profile.traffic.next_reset_date ? `下次 ${profile.traffic.next_reset_date}` : "流量持续累计"}</p></div>
                <p className="text-xs text-muted-foreground">{profile.traffic.reset_days_remaining !== null ? `距下次重置 ${profile.traffic.reset_days_remaining} 天 · 服务端日期` : "由管理员设置重置周期"}</p>
              </Card>
              <Card className="justify-between gap-4 p-5 shadow-sm sm:p-6">
                <div className="flex items-center gap-2 text-sm text-muted-foreground"><CalendarDays className="size-4" />到期时间</div>
                <div className="text-2xl font-semibold tabular-nums tracking-tight">{displayDate(profile.expires_at)}</div>
                <p className="text-xs text-muted-foreground">{profile.expires_at === null ? "账户未设置到期日期" : "请留意账户有效期"}</p>
              </Card>
            </div>
          </section>
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
                <div key={key} className="min-w-0 rounded-xl border bg-muted/20 p-4 transition-colors hover:bg-muted/40 sm:p-5">
                  <div className="font-medium">{title}</div>
                  <div className="mt-1 text-xs text-muted-foreground">{description}</div>
                  <div className="mt-3 flex min-w-0 items-center gap-2 rounded-md border bg-muted/30 px-3 py-2">
                    <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground" title={url}>{url || (error ? "暂时无法加载" : "加载中…")}</span>
                    <Button variant="outline" size="sm" className="shrink-0" disabled={!path} onClick={() => path && void copy(absoluteLink(path), "订阅链接")}><Copy />复制订阅</Button>
                  </div>
                </div>
              )
            })}
          </div>
        </Card>

        <Card className="gap-4 p-5 shadow-sm md:p-6">
          <div>
            <h2 className="flex items-center gap-2 font-semibold">我的节点{proxies && <Badge variant="secondary" className="tabular-nums">{proxies.length}</Badge>}</h2>
            <p className="mt-1 text-sm text-muted-foreground">拖动左侧手柄调整顺序，自动保存并同步到订阅。</p>
          </div>
          {!proxies ? (
            error ? <p className="text-sm text-muted-foreground">节点暂时无法加载，请重试。</p> : <Skeleton className="h-24" />
          ) : proxies.length === 0 ? (
            <p className="rounded-md border border-dashed p-6 text-center text-sm text-muted-foreground">当前没有已启用的节点授权</p>
          ) : (
            <div className="overflow-hidden rounded-md border">
              <table className="w-full table-fixed text-left text-sm">
                <thead className="bg-muted/50 text-xs text-muted-foreground">
                  <tr><th className="w-10 px-1 py-3 sm:px-2"><span className="sr-only">排序</span></th><th className="w-[30%] px-2 py-3 sm:w-[20%] sm:px-3">节点</th><th className="hidden w-[14%] px-2 py-3 sm:table-cell sm:px-3">名称</th><th className="hidden w-[13%] px-3 py-3 xl:table-cell">协议</th><th className="w-[35%] px-2 py-3 sm:w-[22%] sm:px-3">地址</th><th className="hidden w-[7%] px-3 py-3 xl:table-cell">端口</th><th className="hidden w-[12%] px-3 py-3 xl:table-cell">SNI</th><th className="hidden w-[10%] px-3 py-3 xl:table-cell">状态</th><th className="w-20 px-1 py-3 sm:w-24 sm:px-3"><span className="sr-only">复制</span></th></tr>
                </thead>
                <tbody>
                  {drag.order.map((proxy) => (
                    <tr key={proxy.id} {...drag.row(proxy.id)} className="border-t transition-colors hover:bg-muted/30 data-[dragging]:opacity-40">
                      <td className="px-1 py-3 sm:px-2"><DragHandle name={proxy.name} {...drag.handle(proxy.id)} /></td>
                      <td className="whitespace-normal px-2 py-3 sm:px-3">
                        <div className="min-w-0">
                          <div className="font-medium text-balance break-keep">
                            {proxy.node_name}{proxy.node_country && "\u2007"}{proxy.node_country && <Badge variant="outline" className="align-middle font-normal text-muted-foreground">{proxy.node_country}</Badge>}
                          </div>
                          {proxy.node_group && <div className="text-xs text-balance text-muted-foreground">{proxy.node_group}</div>}
                        </div>
                      </td>
                      <td className="hidden px-2 py-3 sm:table-cell sm:px-3"><TooltipText text={proxy.name} className="font-medium" /></td>
                      <td className="hidden px-3 py-3 xl:table-cell"><ProxyProtocolBadge protocol={proxy.protocol} /></td>
                      <td className="px-2 py-3 sm:px-3"><div className="flex min-w-0 items-center gap-1 sm:gap-2"><span className="hidden shrink-0 sm:inline-flex"><ProxyAddressTypeBadge addressType={proxy.address_type} /></span><span className="min-w-0 truncate font-mono text-xs" title={proxy.address}>{proxy.address}</span></div></td>
                      <td className="hidden px-3 py-3 font-mono xl:table-cell">{proxy.port}</td>
                      <td className="hidden px-3 py-3 xl:table-cell"><TooltipText text={proxy.server_name || "—"} className="font-mono text-xs" /></td>
                      <td className="hidden px-3 py-3 xl:table-cell"><Badge variant="outline" className={`font-normal ${proxy.online ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" : "text-muted-foreground"}`}><span className={`size-1.5 rounded-full ${proxy.online ? "bg-emerald-500" : "bg-muted-foreground/50"}`} />{proxy.online ? "在线" : "离线"}</Badge></td>
                      <td className="px-1 py-3 sm:px-3"><div className="flex items-center justify-center gap-0 sm:gap-1"><Button type="button" variant="ghost" size="icon" className="size-8 shrink-0" title="复制 VLESS URL" aria-label={`复制 ${proxy.name} 的 VLESS URL`} onClick={() => void copy(proxy.vless_url, "VLESS URL")}><Link /></Button><Button type="button" variant="ghost" size="icon" className="size-8 shrink-0" title="复制 Mihomo 配置" aria-label={`复制 ${proxy.name} 的 Mihomo 配置`} onClick={() => void copy(proxy.mihomo_yaml, "Mihomo 配置")}><FileCode /></Button></div></td>
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
