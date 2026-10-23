import { useRef, useState } from "react"
import { ChevronRight, Send, Upload } from "lucide-react"
import { toast } from "sonner"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Switch } from "@/components/ui/switch"
import { api, type Node } from "@/lib/api"
import { NodePicker, Field, SwitchRow } from "./Shared"

import { useSettings, siteIcons } from "./hooks"

// `onSaved` refreshes what the header shows, the site name among it.
export function SettingsTab({ onSaved }: { onSaved: () => void }) {
  const { s, set, save } = useSettings()
  const iconPicker = useRef<HTMLInputElement>(null)
  if (!s) return null
  const cdnOnly = s.origin_cdn_only === "on"
  // Applied on its own, as soon as a file is picked: a picked file is already a
  // decision, and the form's save button below is easy to miss for it.
  const saveIcons = (icons: { favicon: string; touch_icon: string }, done: string) =>
    save(icons, done).then((ok) => {
      // The tab's icon is cached under its fixed URL; a new query fetches the
      // one just saved.
      const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]')
      if (ok && link) link.href = `/admin/favicon.svg?${Date.now()}`
    })

  return (
    <div className="space-y-4">
      <Card className="gap-4 p-5">
        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="站点名称">
            <Input value={String(s.site_name ?? "")} onChange={(e) => set("site_name", e.target.value)} placeholder="Monitor" />
          </Field>
          <Field
            label="历史数据保留天数"
            hint="1–365 天，超出的自动清理，累计流量不受影响"
            helpWidth="max-w-66 min-[408px]:max-w-94"
            help={
              <>
                <p>
                  默认 <span className="whitespace-nowrap">30 天</span>，能看约一个月的历史。
                </p>
                <p>
                  最近 <span className="whitespace-nowrap">7 天</span>
                  {"按分钟保存，更早的按小时保存：超过一周的图表上，两者画出来几乎一样，按小时存只占几十分之一的空间。"}
                </p>
                <p>
                  上限 <span className="whitespace-nowrap">365 天</span>。
                </p>
              </>
            }
          >
            <Input
              type="number"
              value={String(s.retention_days ?? "")}
              onChange={(e) => set("retention_days", e.target.value)}
              placeholder="30"
            />
          </Field>
          <Field label="站点图标" hint="标签页、书签和手机主屏幕上的图标，选好即生效，换主题也保留">
            <div className="flex items-center gap-2">
              {!s.favicon && <span className="text-sm text-muted-foreground">默认</span>}
              <div className="flex size-9 shrink-0 items-center justify-center rounded-md border">
                <img src={String(s.favicon || "/favicon.svg?theme")} alt="站点图标" className="size-6 object-contain" />
              </div>
              <Button size="sm" variant="outline" onClick={() => iconPicker.current?.click()}>
                <Upload /> 选择图标
              </Button>
              {s.favicon && (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => saveIcons({ favicon: "", touch_icon: "" }, "已换回主题自带的图标")}
                >
                  用主题自带的
                </Button>
              )}
              <input
                ref={iconPicker}
                type="file"
                accept="image/png,image/x-icon,image/svg+xml,image/webp,image/jpeg,image/gif,.ico"
                className="hidden"
                onChange={(e) => {
                  const file = e.target.files?.[0]
                  e.target.value = ""
                  if (!file) return
                  siteIcons(file).then(
                    (icons) => saveIcons(icons, "站点图标已更新"),
                    (e: Error) => toast.error(e.message),
                  )
                }}
              />
            </div>
          </Field>
          <Field
            label="GitHub 代理"
            hint="留空直连，只在 hub 拉不到 GitHub Release 时填"
            helpWidth="max-w-58 min-[376px]:max-w-86"
            help={<p>agent 有签名校验，主题包没有，只填信得过的镜像。</p>}
          >
            <Input
              value={String(s.github_proxy ?? "")}
              onChange={(e) => set("github_proxy", e.target.value)}
              placeholder="https://ghfast.top"
            />
          </Field>
        </div>
        <div className="space-y-4">
          <SwitchRow
            label="开放公开状态页"
            hint="关闭后所有页面需登录"
            checked={s.public_page !== "off"}
            onChange={(v) => set("public_page", v ? "on" : "off")}
          />
          <SwitchRow
            label="我确认源站只对 Cloudflare 或隧道开放"
            hint="登录通知和限流取访客的真实地址"
            extra={
              <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
                <span>保存后核对，当前认出你的地址</span>
                <code className="rounded-md border bg-muted/50 px-1.5 py-0.5 font-mono text-foreground select-all">
                  {String(s.your_address ?? "未知")}
                </code>
              </p>
            }
            helpWidth="max-w-62 min-[392px]:max-w-90 min-[440px]:max-w-104"
            help={
              <>
                <p>打开前先确认源站只有 Cloudflare 连得上：用隧道，源站不开公网端口；或让 Cloudflare 回源带一个密钥头，反代没带就拒绝。</p>
                <p>只放行 Cloudflare 的回源地址不够，别人的 Worker 也从那些地址连过来。</p>
                <p>
                  不是却打开，别人直连源站自己写 <span className="whitespace-nowrap">CF-Connecting-IP</span> 或{" "}
                  <span className="whitespace-nowrap">X-Forwarded-For</span>，就能每次换一个地址猜密码，限流形同虚设。
                </p>
                <p>别家 CDN 用不上这个开关，在反代上配 realip。</p>
              </>
            }
            checked={cdnOnly}
            onChange={(v) => set("origin_cdn_only", v ? "on" : "off")}
          />
        </div>
        <div>
          <Button
            size="sm"
            onClick={() =>
              save({
                site_name: String(s.site_name ?? ""),
                // `||` rather than `??`: an emptied box saves the default, ""
                // being the one value this key's write path refuses.
                retention_days: String(s.retention_days || "30"),
                github_proxy: String(s.github_proxy ?? ""),
                origin_cdn_only: s.origin_cdn_only === "on" ? "on" : "off",
                public_page: s.public_page === "off" ? "off" : "on",
              }).then((ok) => ok && onSaved())
            }
          >
            保存站点设置
          </Button>
        </div>
      </Card>
    </div>
  )
}

export const TEXT_BOX =
  "w-full min-w-0 rounded-md border border-input bg-transparent px-3 py-2 shadow-xs outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 dark:bg-input/30"
export const TEXTAREA = `${TEXT_BOX} font-mono text-xs`

// One offline alert, filled in the way the hub fills a template: in a single pass,
// JSON-escaped for the webhook body. Previews only; nothing here is sent.
export const SAMPLE_NOTE: Record<string, string> = {
  event: "offline",
  node: "香港 · 甲商家",
  title: "🔴 香港 · 甲商家 离线",
  message: "最后上报 09-15 20:13 +08:00",
  time: "09-15 20:16 +08:00",
}

export const PLACEHOLDERS = "{{title}} {{message}} {{node}} {{event}} {{site}} {{time}}"

export function TemplatePreview({ template, site, json = false }: { template: string; site: string; json?: boolean }) {
  if (!template.trim()) return <p className="text-xs text-muted-foreground">留空保存即恢复默认模板</p>
  const values = { ...SAMPLE_NOTE, site }
  let out = template.replace(/\{\{(event|node|title|message|site|time)\}\}/g, (_, key: keyof typeof values) =>
    json ? JSON.stringify(values[key]).slice(1, -1) : values[key],
  )
  if (json) {
    try {
      out = JSON.stringify(JSON.parse(out), null, 2)
    } catch {
      return (
        <p className="rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
          代入后不是合法 JSON，保存会被拒绝。占位符要写在引号里，例如 "text": "{"{{title}}"}"
        </p>
      )
    }
  }
  return (
    <div className="space-y-1">
      <div className="text-xs text-muted-foreground">预览（以一条离线通知为例）</div>
      <pre className="overflow-x-auto rounded-md bg-muted/50 px-3 py-2 font-mono text-xs whitespace-pre-wrap break-all">{out}</pre>
    </div>
  )
}

// A channel's form, collapsed until needed. The summary carries whether the
// channel is configured, so the closed card still answers the common question.
export function ChannelCard({ title, configured, children }: { title: string; configured: boolean; children: React.ReactNode }) {
  return (
    <Card className="p-5">
      <details className="group">
        <summary className="flex cursor-pointer list-none items-center justify-between gap-3 rounded-md outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 [&::-webkit-details-marker]:hidden">
          <span className="flex items-center gap-2 text-sm font-medium">
            <ChevronRight className="size-4 text-muted-foreground transition-transform group-open:rotate-90" />
            {title}
          </span>
          <Badge variant={configured ? "secondary" : "outline"}>{configured ? "已配置" : "未配置"}</Badge>
        </summary>
        <div className="mt-4 space-y-4">{children}</div>
      </details>
    </Card>
  )
}

// Offline alerts are opt-in per node, so turning them on for a fleet needs one
// place rather than one dialog per node. Ticks are a draft until 保存, like every
// other form in the panel: a request per click would make each tick wait on a
// round trip and a refresh before showing.
export function OfflineNodes({ nodes, refresh }: { nodes: Node[]; refresh: () => void }) {
  // Only the ticks changed here, by node id. A snapshot of every node's state
  // would send back a node another session switched meanwhile.
  const [draft, setDraft] = useState<Map<number, boolean>>(new Map())
  const [saving, setSaving] = useState(false)
  const on = (n: Node) => draft.get(n.id) ?? !!n.notify
  const chosen = new Set(nodes.filter(on).map((n) => n.id))
  // Against the live list, so a node deleted meanwhile is neither counted nor sent.
  const turnOn = nodes.filter((n) => on(n) && !n.notify).map((n) => n.id)
  const turnOff = nodes.filter((n) => !on(n) && n.notify).map((n) => n.id)
  const dirty = turnOn.length + turnOff.length > 0

  // Kept after a save until the list reports it, so the ticks do not flash back
  // to the old state for a round trip. Adjusted during render rather than in an
  // effect, as it follows from props alone.
  if (draft.size && !dirty && !saving) setDraft(new Map())

  const pick = (list: Node[], value: boolean) =>
    setDraft((old) => {
      const next = new Map(old)
      for (const n of list) next.set(n.id, value)
      return next
    })

  async function save() {
    setSaving(true)
    try {
      for (const [ids, on] of [[turnOn, true], [turnOff, false]] as const) {
        if (ids.length) await api("/nodes/batch", { method: "PUT", body: JSON.stringify({ ids, patch: { notify: on } }) })
      }
      toast.success("离线通知已保存")
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      refresh()
      setSaving(false)
    }
  }

  const pending = [turnOn.length && `打开 ${turnOn.length} 台`, turnOff.length && `关闭 ${turnOff.length} 台`].filter(Boolean)
  return (
    <Card className="gap-4 p-5">
      <div>
        <h3 className="text-sm font-medium">离线通知</h3>
        <p className="mt-1 text-xs text-muted-foreground">
          按节点打开，默认关。已打开 {nodes.filter((n) => n.notify).length} / {nodes.length} 台
          {pending.length > 0 && <span className="text-foreground">，待保存：{pending.join("、")}</span>}
        </p>
      </div>
      <NodePicker nodes={nodes} chosen={chosen} onPick={pick} disabled={saving} />
      <div className="flex justify-end gap-2">
        <Button size="sm" variant="ghost" disabled={!dirty || saving} onClick={() => setDraft(new Map())}>撤销</Button>
        <Button size="sm" disabled={!dirty || saving} onClick={save}>保存</Button>
      </div>
    </Card>
  )
}

export function Notify({ nodes, refresh }: { nodes: Node[]; refresh: () => void }) {
  const { s, set, save } = useSettings()
  const [testing, setTesting] = useState(false)
  if (!s) return null
  const text = (k: string) => String(s[k] ?? "")
  // A credential is sent only when something was typed: the field starts empty
  // because the hub never returns the stored value.
  const typed = (...keys: string[]) =>
    Object.fromEntries(keys.filter((k) => typeof s[k] === "string" && s[k] !== "").map((k) => [k, text(k)]))
  const secretHint = (k: string) => (s[`${k}_set`] ? "已设置，留空不变" : "未设置")

  async function test() {
    setTesting(true)
    try {
      const { sent } = await api<{ sent: string[] }>("/notify/test", { method: "POST" })
      toast.success(`测试通知已发送：${sent.join("、")}`)
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setTesting(false)
    }
  }

  return (
    <div className="space-y-4">
      <Card className="gap-4 p-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <h3 className="text-sm font-medium">通知渠道</h3>
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
              Telegram 和 Webhook 配了哪个就发哪个，也可以同时用。离线通知在下方按节点打开；流量和到期提醒对填了额度、到期日的节点生效。
            </p>
          </div>
          <Button size="sm" variant="secondary" disabled={testing} onClick={test}>
            <Send /> {testing ? "发送中…" : "发送测试"}
          </Button>
        </div>
      </Card>

      <ChannelCard title="Telegram" configured={!!s.notify_telegram_token_set && text("notify_telegram_chat") !== ""}>
        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="Bot Token" hint={secretHint("notify_telegram_token")}>
            <Input
              type="password"
              autoComplete="off"
              placeholder={s.notify_telegram_token_set ? "••••••••" : "123456:ABC-DEF…"}
              value={text("notify_telegram_token")}
              onChange={(e) => set("notify_telegram_token", e.target.value)}
            />
          </Field>
          <Field label="Chat ID" hint="数字 ID，群组是负数；公开频道可填 @频道名">
            <Input value={text("notify_telegram_chat")} onChange={(e) => set("notify_telegram_chat", e.target.value)} placeholder="-1001234567890" />
          </Field>
        </div>
        <Field label="消息模板" hint={`纯文本。占位符 ${PLACEHOLDERS}`}>
          <textarea rows={3} className={TEXTAREA} value={text("notify_telegram_text")} onChange={(e) => set("notify_telegram_text", e.target.value)} />
        </Field>
        <TemplatePreview template={text("notify_telegram_text")} site={text("site_name") || "Monitor"} />
        <div className="flex gap-2">
          <Button
            size="sm"
            onClick={() =>
              save({
                notify_telegram_chat: text("notify_telegram_chat"),
                notify_telegram_text: text("notify_telegram_text"),
                ...typed("notify_telegram_token"),
              })
            }
          >
            保存 Telegram
          </Button>
          {s.notify_telegram_token_set && (
            <Button size="sm" variant="ghost" onClick={() => save({ notify_telegram_token: "", notify_telegram_chat: "" }, "已清除 Telegram")}>
              清除
            </Button>
          )}
        </div>
      </ChannelCard>

      <ChannelCard title="Webhook" configured={!!s.notify_webhook_url_set}>
        <Field label="URL" hint={secretHint("notify_webhook_url")}>
          <Input
            type="password"
            autoComplete="off"
            placeholder={s.notify_webhook_url_set ? "••••••••" : "https://…"}
            value={text("notify_webhook_url")}
            onChange={(e) => set("notify_webhook_url", e.target.value)}
          />
        </Field>
        <Field label="请求头" hint={`可选，一行一个。${s.notify_webhook_headers_set ? "已设置，留空不变" : ""}`}>
          <textarea
            rows={2}
            className={TEXTAREA}
            placeholder={s.notify_webhook_headers_set ? "••••••••" : "Authorization: Bearer xxx"}
            value={text("notify_webhook_headers")}
            onChange={(e) => set("notify_webhook_headers", e.target.value)}
          />
        </Field>
        <Field label="请求体" hint={`以 POST 发送，Content-Type 为 application/json。占位符 ${PLACEHOLDERS}，须写在引号内`}>
          <textarea rows={4} className={TEXTAREA} value={text("notify_webhook_body")} onChange={(e) => set("notify_webhook_body", e.target.value)} />
        </Field>
        <TemplatePreview template={text("notify_webhook_body")} site={text("site_name") || "Monitor"} json />
        <div className="flex gap-2">
          <Button
            size="sm"
            onClick={() => save({ notify_webhook_body: text("notify_webhook_body"), ...typed("notify_webhook_url", "notify_webhook_headers") })}
          >
            保存 Webhook
          </Button>
          {s.notify_webhook_headers_set && (
            <Button size="sm" variant="ghost" onClick={() => save({ notify_webhook_headers: "" }, "已清除请求头")}>
              清除请求头
            </Button>
          )}
          {s.notify_webhook_url_set && (
            <Button size="sm" variant="ghost" onClick={() => save({ notify_webhook_url: "", notify_webhook_headers: "" }, "已清除 Webhook")}>
              清除
            </Button>
          )}
        </div>
      </ChannelCard>

      <OfflineNodes nodes={nodes} refresh={refresh} />

      <Card className="gap-4 p-5">
        <h3 className="text-sm font-medium">事件</h3>
        <div className="grid gap-4 sm:grid-cols-3">
          <Field label="离线宽限期（分钟）" hint="断开超过这么久才算离线，1–30">
            <Input type="number" min={1} max={30} value={text("notify_grace")} onChange={(e) => set("notify_grace", e.target.value)} />
          </Field>
          <Field label="流量提醒（%）" hint="本期用量达到该比例和 100% 时各提醒一次，0 关闭">
            <Input type="number" min={0} max={100} value={text("notify_traffic")} onChange={(e) => set("notify_traffic", e.target.value)} />
          </Field>
          <Field label="到期提醒（天）" hint="每天 9 点汇总这么多天内到期的节点，自动续期时也提醒，0 关闭">
            <Input type="number" min={0} max={365} value={text("notify_expiry")} onChange={(e) => set("notify_expiry", e.target.value)} />
          </Field>
        </div>
        <div className="flex items-center gap-2 text-sm">
          <Switch aria-labelledby="notify-login-label" checked={s.notify_login !== "off"} onCheckedChange={(v) => set("notify_login", v ? "on" : "off")} />
          <span id="notify-login-label">登录后台时提醒</span>
        </div>
        <div>
          <Button
            size="sm"
            onClick={() =>
              save({
                notify_grace: text("notify_grace"),
                notify_traffic: text("notify_traffic"),
                notify_expiry: text("notify_expiry"),
                notify_login: s.notify_login === "off" ? "off" : "on",
              })
            }
          >
            保存事件设置
          </Button>
        </div>
      </Card>
    </div>
  )
}

// The two ways into this panel, on their own page: the GitHub identity it trusts
// and the password that works when GitHub does not.
