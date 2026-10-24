import { useEffect, useRef, useState } from "react"
import { CalendarClock, Copy, Download, Layers, Pencil, Plus, Server, Trash2 } from "lucide-react"
import { toast } from "sonner"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { AdminConfirmDialog as ConfirmDialog, AdminSearchInput } from "@/components/AdminShared"
import { DragHandle, useDragOrder } from "@/components/DragOrder"
import { api, badIfaceName, changes, currentIface, GIB, ifaceChoice, ifaceSpec, inGroup, provisioningSite, trafficCorrection, type IfaceChoice, type Node } from "@/lib/api"
import { realmArg } from "@/lib/realm"
import { bytes, cycleMonths, FOREVER, money, uptime } from "@/lib/format"
import { TRAFFIC_FIELDS, TRAFFIC_MODES, nameText, copy, Addresses, searchNodes, useGroupFilter, GroupFilter, NodePicker, Command, Field, OptionRow, GroupInput } from "./Shared"
import { type Settings, useSettings } from "./hooks"

export function GroupDialog({ nodes, onClose, onSaved }: { nodes: Node[]; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState("")
  const [chosen, setChosen] = useState<Set<number>>(new Set())
  const [saving, setSaving] = useState(false)
  const group = name.trim()
  // Counted against the live list, so a node deleted meanwhile is not sent.
  const ids = nodes.filter((n) => chosen.has(n.id)).map((n) => n.id)
  const pick = (list: Node[], on: boolean) =>
    setChosen((old) => {
      const next = new Set(old)
      for (const n of list) {
        if (on) next.add(n.id)
        else next.delete(n.id)
      }
      return next
    })

  async function save() {
    setSaving(true)
    try {
      await api("/nodes/batch", { method: "PUT", body: JSON.stringify({ ids, patch: { group } }) })
      toast.success(group ? `已把 ${ids.length} 台设为「${group}」` : `已把 ${ids.length} 台移出分组`)
      onClose()
      onSaved()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent onOpenAutoFocus={(e) => e.preventDefault()} className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>设置分组</DialogTitle>
          <DialogDescription className="leading-relaxed">
            勾选节点，设为同一个分组。改名或解散：先筛选出这个分组、全选，再填新名字或清空。
          </DialogDescription>
        </DialogHeader>
        <form noValidate className="contents" onSubmit={(e) => { e.preventDefault(); save() }}>
          <div className="space-y-4">
            <Field label="分组名" hint="公开页可见，最多 13 字；留空为移出分组">
              <GroupInput nodes={nodes} value={name} onChange={setName} />
            </Field>
            <NodePicker nodes={nodes} chosen={chosen} onPick={pick} />
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving || !ids.length} className="max-w-full gap-0">
              <span className="truncate">{group ? `设为「${group}」` : "移出分组"}</span>
              {ids.length > 0 && <span className="tnum shrink-0">（{ids.length} 台）</span>}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

export function CreateNode({ onClose, onSaved }: {
  onClose: () => void
  onSaved: (id: number) => void
}) {
  const [name, setName] = useState("")
  const [saving, setSaving] = useState(false)

  async function save(e: React.FormEvent) {
    e.preventDefault()
    if (!name.trim()) return toast.error("请填写节点名称")
    setSaving(true)
    try {
      const { id } = await api<{ id: number }>("/nodes", {
        method: "POST",
        body: JSON.stringify({ name: name.trim() }),
      })
      toast.success("节点已添加")
      onClose()
      onSaved(id)
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>添加节点</DialogTitle>
        </DialogHeader>
        <form className="space-y-4" onSubmit={save}>
          <Field label="名称">
            <Input autoFocus maxLength={64} value={name} onChange={(e) => setName(e.target.value)} placeholder="香港 · 甲商家" />
          </Field>
          <DialogFooter className="border-t pt-4">
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving}>添加</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

export function NodeForm({ node, nodes, onClose, onSaved }: {
  node: Node
  nodes: Node[]
  onClose: () => void
  onSaved: () => void
}) {
  const [form, setForm] = useState(node)
  const [limitGib, setLimitGib] = useState(String(node.traffic_limit / GIB || ""))
  // Text, as the limit is: a number state turns an emptied box into 0.
  const [resetDay, setResetDay] = useState(String(node.traffic_reset_day))
  const [saving, setSaving] = useState(false)
  const gib = (bytes: number) => String(Number((bytes / GIB).toFixed(3)))
  const [traffic, setTraffic] = useState(() =>
    Object.fromEntries(TRAFFIC_FIELDS.map(([k]) => [k, gib(node[k])])) as Record<string, string>,
  )
  // Compared as entered rather than as bytes: rounding to GB would read as an
  // edit and zero a node that has transferred a few MB.
  const pristine = useRef(traffic)
  const set = <K extends keyof Node>(k: K, v: Node[K]) => setForm((f) => ({ ...f, [k]: v }))
  // What each address box falls back to when left empty.
  const automatic = (v6: boolean) => (v6 ? node.ipv6_auto : node.ipv4_auto) || "无"

  async function save() {
    if (!form.name.trim()) return toast.error("请填写节点名称")
    const resetOn = Number(resetDay)
    if (!Number.isInteger(resetOn) || resetOn < 1 || resetOn > 31) return toast.error("每月重置日要填 1–31 之间的整数")
    const patch = changes(node, {
      name: form.name.trim(),
      public: form.public,
      remark: form.remark,
      public_remark: (form.public_remark ?? "").trim(),
      group: (form.group ?? "").trim(),
      traffic_mode: form.traffic_mode,
      traffic_limit: Math.round(Number(limitGib) * GIB),
      traffic_reset_day: resetOn,
      notify: !!form.notify,
      ipv4_pin: (form.ipv4_pin ?? "").trim(),
      ipv6_pin: (form.ipv6_pin ?? "").trim(),
      country_pin: (form.country_pin ?? "").trim().toUpperCase(),
    })
    const correction = trafficCorrection(pristine.current, traffic)
    if ([patch.traffic_limit, ...Object.values(correction)].some((v) => v !== undefined && (!Number.isSafeInteger(v) || v < 0))) {
      return toast.error("流量必须是有效的非负数，且不能超出精确计数范围")
    }
    setSaving(true)
    try {
      // The correction belongs to the new reset period, so its day is saved
      // first.
      if (Object.keys(patch).length) {
        await api(`/nodes/${node.id}`, { method: "PUT", body: JSON.stringify(patch) })
      }
      if (Object.keys(correction).length) {
        await api(`/nodes/${node.id}/traffic`, {
          method: "PUT",
          body: JSON.stringify(correction),
        })
      }
      toast.success("节点设置已保存")
      onClose()
      onSaved()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent onOpenAutoFocus={(e) => e.preventDefault()} className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{node.name}</DialogTitle>
        </DialogHeader>
        {/* noValidate here and in the other dialogs: save() checks every field.
            The browser's own check would refuse a fractional GB against the
            default step of 1, and cannot point at a field folded inside
            流量校正, so the save button would do nothing. */}
        <form noValidate className="contents" onSubmit={(e) => { e.preventDefault(); save() }}>
          <div className="space-y-6">
            <section className="space-y-4">
              <div className="grid gap-4 sm:grid-cols-2">
                <Field label="名称">
                  <Input maxLength={64} value={form.name} onChange={(e) => set("name", e.target.value)} />
                </Field>
                <Field label="分组" hint="公开页可见，留空为未分组">
                  <GroupInput nodes={nodes} value={form.group ?? ""} onChange={(v) => set("group", v)} />
                </Field>
                <Field label="公开备注">
                  <Input
                    value={form.public_remark ?? ""}
                    onChange={(e) => set("public_remark", e.target.value)}
                    placeholder="公开页可见，最多 100 字"
                  />
                </Field>
                <Field label="私有备注">
                  <Input value={form.remark ?? ""} onChange={(e) => set("remark", e.target.value)} placeholder="仅管理员可见" />
                </Field>
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                <OptionRow title="公开显示" hint="关闭后只在管理后台可见" toggle>
                  <Switch checked={form.public} onCheckedChange={(v) => set("public", v)} />
                </OptionRow>
                <OptionRow title="离线通知" hint="掉线超过宽限期、恢复时各推一条" toggle>
                  <Switch checked={!!form.notify} onCheckedChange={(v) => set("notify", v)} />
                </OptionRow>
              </div>
            </section>
            <section className="space-y-3 border-t pt-5">
              <h3 className="text-sm font-medium">流量</h3>
              {/* On a phone the two short numbers share a row, the mode takes
                  the next; dense packing restores the order from sm up. */}
              <div className="grid grid-flow-row-dense grid-cols-2 gap-4 sm:grid-cols-3">
                <Field label="每月额度 (GB)" hint="留空或 0 不限">
                  <Input type="number" value={limitGib} onChange={(e) => setLimitGib(e.target.value)} placeholder="1024" />
                </Field>
                <Field label="计算方式" className="col-span-2 sm:col-span-1">
                  <Select value={form.traffic_mode} onValueChange={(v) => set("traffic_mode", v)}>
                    <SelectTrigger className="w-full"><SelectValue /></SelectTrigger>
                    <SelectContent position="popper">
                      {Object.entries(TRAFFIC_MODES).map(([k, v]) => (
                        <SelectItem key={k} value={k}>{v}</SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="每月重置日" hint="1–31，改后本月重算，总量不变">
                  <Input type="number" min={1} max={31} value={resetDay} onChange={(e) => setResetDay(e.target.value)} />
                </Field>
              </div>
              <details className="rounded-lg border bg-muted/30 px-3 py-2.5">
                <summary className="cursor-pointer text-sm font-medium">流量校正</summary>
                <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
                  按 GB 填入需要校正的值，未修改的计数器继续正常累计。
                </p>
                <div className="mt-3 grid grid-cols-2 gap-4">
                  {TRAFFIC_FIELDS.map(([key, label]) => (
                    <Field key={key} label={`${label} (GB)`}>
                      <Input
                        type="number"
                        step="0.001"
                        value={traffic[key]}
                        onChange={(e) => setTraffic((t) => ({ ...t, [key]: e.target.value }))}
                      />
                    </Field>
                  ))}
                </div>
              </details>
            </section>
            <section className="space-y-3 border-t pt-5">
              <h3 className="text-sm font-medium">地址与地区</h3>
              <div className="grid grid-flow-row-dense grid-cols-[1fr_5rem] gap-4 sm:grid-cols-[1fr_1.4fr_6rem]">
                <Field label="IPv4">
                  <Input value={form.ipv4_pin ?? ""} onChange={(e) => set("ipv4_pin", e.target.value)} placeholder={`自动：${automatic(false)}`} />
                </Field>
                <Field label="IPv6" className="col-span-full sm:col-span-1">
                  <Input value={form.ipv6_pin ?? ""} onChange={(e) => set("ipv6_pin", e.target.value)} placeholder={`自动：${automatic(true)}`} />
                </Field>
                <Field label="国家/地区">
                  <Input
                    value={form.country_pin ?? ""}
                    maxLength={2}
                    onChange={(e) => set("country_pin", e.target.value.toUpperCase())}
                    placeholder={`自动：${node.country_auto || "无"}`}
                  />
                </Field>
              </div>
              <p className="text-xs leading-relaxed text-muted-foreground">
                留空为自动。国家/地区填两位代码，如 CN；手填的值会一直显示，IP 变了要自己改。
              </p>
            </section>
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving}>保存</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

export const CURRENCY_NAMES = new Intl.DisplayNames(["zh-CN"], { type: "currency" })

// The name confirms a code the hub can only check the shape of. DisplayNames
// echoes back a code outside ISO 4217 and throws on anything but three letters,
// which the hub refuses with its own message.
export function currencyHint(code: string) {
  try {
    const name = CURRENCY_NAMES.of(code)
    return name === code ? "未知代码，照原样显示" : name
  } catch {
    return undefined
  }
}

export function BillingForm({ node, onClose, onSaved }: {
  node: Node
  onClose: () => void
  onSaved: () => void
}) {
  const [form, setForm] = useState(node)
  // Text rather than a number: a numeric state cannot represent an empty field,
  // so clearing it would snap back to 0 mid-entry. Empty means free.
  const [price, setPrice] = useState(node.price > 0 ? String(node.price) : "")
  // Whole years are entered in years, the way a five-year plan is sold.
  const months = cycleMonths(node.billing_cycle)
  const [unit, setUnit] = useState(months === 0 ? "once" : months % 12 ? "months" : "years")
  const [count, setCount] = useState(String(months % 12 ? months : months / 12 || 1))
  const [saving, setSaving] = useState(false)
  const set = <K extends keyof Node>(k: K, v: Node[K]) => setForm((f) => ({ ...f, [k]: v }))

  async function save() {
    // The hub refuses a length out of range and stores a named one by name, so
    // an unchanged length is compared in months, not in spelling.
    const cycle = unit === "once" ? "once" : `${Number(count) * (unit === "years" ? 12 : 1)}m`
    setSaving(true)
    try {
      await api(`/nodes/${node.id}`, {
        method: "PUT",
        body: JSON.stringify(changes(node, {
          price: Math.max(0, Number(price) || 0),
          currency: form.currency,
          billing_cycle: cycleMonths(cycle) === months ? node.billing_cycle : cycle,
          expires_at: form.expires_at || null,
        })),
      })
      toast.success("续费设置已保存")
      onClose()
      onSaved()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent onOpenAutoFocus={(e) => e.preventDefault()} className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>{node.name}</DialogTitle>
        </DialogHeader>
        <form noValidate className="contents" onSubmit={(e) => { e.preventDefault(); save() }}>
          <div className="space-y-5">
            <div className="grid grid-cols-2 gap-4">
              <Field label="价格" hint="留空或 0 为免费">
                <Input
                  type="number"
                  min="0"
                  step="0.01"
                  value={price}
                  onChange={(e) => setPrice(e.target.value)}
                  placeholder="免费"
                />
              </Field>
              <Field
                label="货币"
                hint={currencyHint(form.currency.toUpperCase())}
                helpWidth="max-w-72"
                help={
                  <>
                    <p>填三个字母的货币代码，大小写都行。</p>
                    <p>
                      例如：
                      {["美元 USD", "人民币 CNY", "港币 HKD", "新台币 TWD", "欧元 EUR", "日元 JPY"].map((c, i) => (
                        <span key={c}>{i > 0 && "、"}<span className="whitespace-nowrap">{c}</span></span>
                      ))}
                    </p>
                  </>
                }
              >
                {/* Uppercased by CSS: rewriting the value mid-composition would
                    break an input method, and the hub stores it uppercased. */}
                <Input
                  maxLength={3}
                  autoCapitalize="characters"
                  spellCheck={false}
                  className="uppercase"
                  value={form.currency}
                  onChange={(e) => set("currency", e.target.value)}
                  placeholder="USD"
                />
              </Field>
            </div>
            <div className="grid grid-cols-2 gap-4">
              <Field label="付款周期">
                <div className="flex gap-2">
                  {unit !== "once" && (
                    <Input
                      type="number"
                      min="1"
                      step="1"
                      aria-label="周期长度"
                      className="w-16"
                      value={count}
                      onChange={(e) => setCount(e.target.value)}
                    />
                  )}
                  <Select value={unit} onValueChange={setUnit}>
                    <SelectTrigger className="min-w-0 flex-1"><SelectValue /></SelectTrigger>
                    <SelectContent position="popper">
                      <SelectItem value="months">月</SelectItem>
                      <SelectItem value="years">年</SelectItem>
                      <SelectItem value="once">一次性</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </Field>
              <Field label="到期时间">
                <Input type="date" value={form.expires_at ?? ""} onChange={(e) => set("expires_at", e.target.value)} />
              </Field>
            </div>
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onClose}>取消</Button>
            <Button type="submit" disabled={saving}>保存</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

// Each command runs the hub's own install.sh and is offered only on an https
// domain entry. `args` receives that entry, which the agent is also given as
// --server.
export function scriptCommand(site: string, args: (site: string) => string[]) {
  site = provisioningSite(site)
  return site && `curl -fsSL ${site}/install.sh | sh -s -- ${args(site).join(" ")}`
}

// Built here rather than fetched: the node list already carries the token, so
// viewing an install command is a read rather than an action. Reissuing one to
// display it would take the running agent offline.
export function installCommand(site: string, token: string, seconds: number | undefined, iface: string | undefined, realm?: boolean, mieru?: boolean, singbox = true) {
  return scriptCommand(site, (s) => [`--server ${s}`, `--token ${token}`, ...intervalArg(seconds), ...ifaceArg(iface), ...realmArg(realm), ...(mieru === undefined ? [] : [mieru ? "--mieru" : "--no-mieru"]), ...(singbox ? ["--sing-box"] : ["--skip-sing-box"])])
}

function RealmOption({ on, setOn }: { on: boolean; setOn: (on: boolean) => void }) {
  return <OptionRow title="安装 Realm" hint="开启后安装，配置转发规则后手动启动。关闭后执行命令会卸载本安装程序管理的 Realm，配置保留。">
    <Switch checked={on} onCheckedChange={setOn} aria-label="安装 Realm" />
  </OptionRow>
}

export function intervalArg(seconds: number | undefined) {
  return seconds === undefined ? [] : [`--interval ${seconds}`]
}

// '' is how install.sh is told to clear a value it would otherwise keep; a
// name needs no quoting, as ifaceSpec admits no shell metacharacter.
export function ifaceArg(iface: string | undefined) {
  return iface === undefined ? [] : [`--iface ${iface || "''"}`]
}

// One command for a batch of machines. The key belongs to the hub, is valid only
// within the window it opened, and each machine exchanges it for a token of its
// own, so unlike an install command this text is no one's credential and can be
// sent to every machine as it is.
export function registerCommand(site: string, key: string, seconds: number | undefined, iface: string | undefined, realm?: boolean, mieru?: boolean, singbox = true) {
  return scriptCommand(site, (s) => [`--server ${s}`, `--register ${key}`, ...intervalArg(seconds), ...ifaceArg(iface), ...realmArg(realm), ...(mieru === undefined ? [] : [mieru ? "--mieru" : "--no-mieru"]), ...(singbox ? ["--sing-box"] : ["--skip-sing-box"])])
}

// Carries no token, so it is the same for every node and remains valid after the
// node is deleted.
export function uninstallCommand(site: string) {
  return scriptCommand(site, () => ["--uninstall"])
}

// Also the same for every node: install.sh reads the token and the hub address
// from the machine's own env file.
export function upgradeCommand(site: string) {
  return scriptCommand(site, () => ["--upgrade"])
}

// The window lives on the hub; this reads it back and counts down, which is also
// what makes an expired one disappear from the panel without interaction.
export function useRegisterWindow() {
  const [key, setKey] = useState("")
  const [until, setUntil] = useState(0)
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000))
  // Advanced by every read sent and every change that lands, so an answer
  // overtaken by either is dropped rather than restoring a replaced key.
  const epoch = useRef(0)
  const begin = (key: string, left: number) => {
    const at = Math.floor(Date.now() / 1000)
    setKey(key)
    setNow(at)
    setUntil(at + left)
  }
  // Also run as the dialog opens: the window may have been closed or reopened
  // from another device since this page loaded, and the command shown must
  // carry the key the hub holds now.
  const sync = () => {
    const at = ++epoch.current
    api<Settings>("/settings")
      .then((s) => at === epoch.current && begin(String(s.register_key ?? ""), Number(s.register_left ?? 0)))
      .catch(() => {})
  }

  useEffect(() => {
    sync()
    const timer = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000)
    return () => clearInterval(timer)
  }, [])

  return {
    key,
    left: key === "" ? 0 : Math.max(0, until - now),
    sync,
    async open() {
      try {
        const w = await api<{ register_key: string; register_left: number }>("/register-window", { method: "POST" })
        epoch.current++
        begin(w.register_key, w.register_left)
      } catch (e) {
        toast.error((e as Error).message)
      }
    },
    async close() {
      try {
        await api("/register-window", { method: "DELETE" })
        epoch.current++
        setKey("")
        setUntil(0)
        toast.success("注册窗口已关闭")
      } catch (e) {
        toast.error((e as Error).message)
      }
    },
  }
}

export function RegisterDialog({ site, reg, onClose }: {
  site: string
  reg: ReturnType<typeof useRegisterWindow>
  onClose: () => void
}) {
  const iface = useIfaceOption(undefined)
  const interval = useIntervalOption()
  const [realm, setRealm] = useState(false)
  const [mieru, setMieru] = useState<boolean | undefined>(undefined)
  const [singbox, setSingbox] = useState(true)
  const command = reg.left > 0 && iface.valid ? registerCommand(site, reg.key, interval.flag, iface.flag, realm, mieru, singbox) : ""
  const clock = `${Math.floor(reg.left / 60)}:${String(reg.left % 60).padStart(2, "0")}`

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent onOpenAutoFocus={(e) => e.preventDefault()} className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>批量添加</DialogTitle>
        </DialogHeader>
        <div className="space-y-5">
          {/* One string: JSX turns a line break inside CJK text into a visible space. */}
          <p className="text-sm text-muted-foreground">
            {"开一个一小时的注册窗口。期间这条命令在任意机器上跑一次，那台机器就会自己出现在列表里，" +
              "名字默认是「节点 N」（N 是节点编号）。命令里没有任何一台机器的凭证，可以同时发给多台机器。"}
          </p>
          <section className="space-y-3">
            <h3 className="text-sm font-medium">安装选项</h3>
            <IntervalOption option={interval} batch />
            <IfaceOption option={iface} batch />
            <RealmOption on={realm} setOn={setRealm} />
            <OptionRow title="安装 mieru" hint="开启安装 mita，配置及授权后启动；切换为关闭会卸载托管程序，配置保留。未操作时保留原状态。"><Switch checked={mieru ?? false} onCheckedChange={setMieru} aria-label="安装 mieru" /></OptionRow>
            <OptionRow title="安装 sing-box" hint="关闭跳过安装与启动，已有程序保留；重新开启会恢复安装管理。"><Switch checked={singbox} onCheckedChange={setSingbox} aria-label="安装 sing-box" /></OptionRow>
          </section>
          {reg.left > 0 ? (
            <section className="space-y-2 border-t pt-5">
              <h3 className="text-sm font-medium">安装命令</h3>
              <Command className={`max-h-40 min-h-24 ${command ? "" : "text-muted-foreground"}`}>
                {command || "网卡名有误，改正后显示命令"}
              </Command>
              {/* Per machine, so it cannot be part of the one command. */}
              <p className="text-xs leading-relaxed text-muted-foreground">
                要给某台单独起名，在它执行的命令末尾加 <code>--name 名字</code>，只对新建的节点生效。
                <a
                  className="ml-1 underline underline-offset-2 hover:text-foreground"
                  href="https://monitor-document.pages.dev/install/batch"
                  target="_blank"
                  rel="noreferrer"
                >
                  批量执行的做法
                </a>
              </p>
              <OptionRow title={`窗口 ${clock} 后自动关闭`} hint="到点自动失效，装完了也可以现在就关">
                <Button variant="outline" size="sm" onClick={reg.close}>立即关闭</Button>
              </OptionRow>
            </section>
          ) : (
            <Button onClick={reg.open}>开启一小时窗口</Button>
          )}
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={onClose}>关闭</Button>
          <Button onClick={() => copy(command)} disabled={!command}>
            <Copy className="size-4" /> 复制
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// The reporting interval both install dialogs offer, opening on the node's
// current one where the hub has read it from the reports. `flag` stays
// undefined until the field is changed, including back to what it opened on.
export function useIntervalOption(current?: number | null) {
  const [typed, setTyped] = useState<string>()
  const text = typed ?? String(current ?? 1)
  const seconds = Math.min(3600, Math.max(1, Math.round(Number(text) || 1)))
  return { typed: text, setTyped, current, flag: typed === undefined ? undefined : seconds }
}

export function IntervalOption({ option, batch = false }: { option: ReturnType<typeof useIntervalOption>; batch?: boolean }) {
  return (
    <OptionRow
      title="上报间隔"
      hint={batch ? "1–3600 秒，默认 1 秒。这一批机器都按这个间隔上报，机器多时可以调大" : (
        <>
          1–3600 秒，默认 1 秒。不改动时沿用机器上原有的间隔
          {option.current && <span className="mt-0.5 block">当前：{option.current} 秒</span>}
        </>
      )}
    >
      <span className="flex shrink-0 items-center gap-2 text-muted-foreground">
        {/* Text rather than number: no spinner arrows, and no wheel changing
            the value under a passing scroll. */}
        <Input
          inputMode="numeric"
          value={option.typed}
          onChange={(e) => option.setTyped(e.target.value.replace(/\D/g, ""))}
          aria-label="上报间隔（秒）"
          className="tnum h-8 w-20 bg-background text-right"
        />
        秒
      </span>
    </OptionRow>
  )
}

// The `--iface` part of an install command. Off leaves the flag out, and
// install.sh then keeps whatever the machine already has; on with both lists
// empty passes '' and restores the default rules. It opens on for a node whose
// agent reports a list, so reinstalling from here repeats that list rather than
// relying on the machine to remember it.
export function useIfaceOption(current: string | undefined) {
  const [on, setOn] = useState(!!current)
  const [choice, setChoice] = useState<IfaceChoice>(() => ifaceChoice(current ?? ""))
  const bad = on ? badIfaceName(choice) : undefined
  const spec = ifaceSpec(choice)
  return { on, setOn, choice, setChoice, current, bad, valid: !bad, flag: on && spec !== null ? spec : undefined }
}

export function describeIface(spec: string) {
  const { only, skip } = ifaceChoice(spec)
  const parts = [only && `只统计 ${only.replaceAll(",", "、")}`, skip && `不统计 ${skip.replaceAll(",", "、")}`]
  return parts.filter(Boolean).join("；") || "默认规则"
}

export function IfaceOption({ option, batch = false }: { option: ReturnType<typeof useIfaceOption>; batch?: boolean }) {
  const { on, setOn, choice, setChoice, current, bad } = option
  const field = (list: keyof IfaceChoice, label: string, placeholder: string) => (
    <Field label={label}>
      <Input
        value={choice[list]}
        onChange={(e) => setChoice({ ...choice, [list]: e.target.value })}
        placeholder={placeholder}
        spellCheck={false}
        aria-invalid={bad?.list === list}
        className="bg-background font-mono text-xs"
      />
    </Field>
  )
  const hint = on
    ? batch ? "每台机器都按这里的设置统计" : "覆盖这台机器原有的设置"
    : batch ? "关闭时各台机器沿用原有设置，新机器按默认规则" : "关闭时沿用机器上原有的设置，转发流量的机器才需要指定"
  return (
    <OptionRow
      title="指定统计的网卡"
      hint={<>{hint}{current !== undefined && <span className="mt-0.5 block">当前：{describeIface(current)}</span>}</>}
      toggle
      below={on && (
        <div className="space-y-2.5">
          <div className="grid gap-3 sm:grid-cols-2">
            {field("only", "只统计", "如 WAN 口 eth1 或 pppoe-wan")}
            {field("skip", "不统计", "如 LAN 口 eth0")}
          </div>
          <p className={`text-xs leading-relaxed ${bad ? "text-destructive" : "text-muted-foreground"}`}>
            {bad
              ? `「${bad.name}」不是有效的网卡名：写完整的名字，多个用逗号分隔`
              : "写完整的网卡名，多个用逗号分隔。两项都留空即恢复默认规则。"}
          </p>
        </div>
      )}
    >
      <Switch checked={on} onCheckedChange={setOn} />
    </OptionRow>
  )
}

export function InstallDialog({ node, site, onClose, onRotated }: {
  node: Node
  site: string
  onClose: () => void
  onRotated: () => void
}) {
  const [token, setToken] = useState("")
  const [unread, setUnread] = useState("")
  const [rotating, setRotating] = useState(false)
  const [confirmRotate, setConfirmRotate] = useState(false)
  const iface = useIfaceOption(currentIface(node))
  const interval = useIntervalOption(node.interval)
  const [realm, setRealm] = useState(false)
  const [mieru, setMieru] = useState<boolean | undefined>(undefined)
  const [singbox, setSingbox] = useState(true)

  // Read as the dialog opens rather than carried in the node list, where it
  // would share an answer with strings agents report. A token rotated meanwhile
  // is newer and kept.
  useEffect(() => {
    let live = true
    api<{ token: string }>(`/nodes/${node.id}/token`)
      .then((t) => live && setToken((held) => held || t.token))
      .catch((e) => live && setUnread((e as Error).message))
    return () => { live = false }
  }, [node.id])

  const command = token && iface.valid ? installCommand(site, token, interval.flag, iface.flag, realm, mieru, singbox) : ""
  const placeholder = token ? "网卡名有误，改正后显示命令" : unread ? `读取凭证失败：${unread}` : "正在读取凭证…"

  async function rotate() {
    setRotating(true)
    try {
      const fresh = await api<{ token: string }>(`/nodes/${node.id}/token`, { method: "POST" })
      setToken(fresh.token)
      setUnread("")
      setConfirmRotate(false)
      toast.success("凭证已换发，需用新命令重装")
      onRotated()
    } catch (e) {
      toast.error((e as Error).message)
      // The rotation may have taken effect with its answer lost on the way, and
      // the command shown would then carry a retired token. Whichever holds now.
      api<{ token: string }>(`/nodes/${node.id}/token`).then((t) => setToken(t.token)).catch(() => {})
    } finally {
      setRotating(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent onOpenAutoFocus={(e) => e.preventDefault()} className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{node.name}</DialogTitle>
          {/* The one place a single node's agent version is shown, and what an
              issue report asks for. Empty until the node has reported once. */}
          {node.agent_version && <DialogDescription>当前 agent v{node.agent_version}</DialogDescription>}
        </DialogHeader>
        <div className="space-y-5">
          <section className="space-y-3">
            <h3 className="text-sm font-medium">安装选项</h3>
            <IntervalOption option={interval} />
            <IfaceOption option={iface} />
            <RealmOption on={realm} setOn={setRealm} />
            <OptionRow title="安装 mieru" hint="开启安装 mita，配置及授权后启动；切换为关闭会卸载托管程序，配置保留。未操作时保留原状态。"><Switch checked={mieru ?? false} onCheckedChange={setMieru} aria-label="安装 mieru" /></OptionRow>
            <OptionRow title="安装 sing-box" hint="关闭跳过安装与启动，已有程序保留；重新开启会恢复安装管理。"><Switch checked={singbox} onCheckedChange={setSingbox} aria-label="安装 sing-box" /></OptionRow>
          </section>
          <section className="space-y-2 border-t pt-5">
            <h3 className="text-sm font-medium">安装命令</h3>
            <Command className={`max-h-40 min-h-24 ${command ? "" : "text-muted-foreground"}`}>
              {command || placeholder}
            </Command>
          </section>
          <OptionRow title="换发凭证" hint="旧凭证立即作废，agent 掉线，需用新命令重装">
            <Button variant="outline" size="sm" disabled={rotating} onClick={() => setConfirmRotate(true)}>
              换发
            </Button>
          </OptionRow>
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={onClose}>关闭</Button>
          <Button onClick={() => copy(command)} disabled={!command}>
            <Copy className="size-4" /> 复制
          </Button>
        </DialogFooter>
      </DialogContent>
      {confirmRotate && (
        <ConfirmDialog
          title={`给「${node.name}」换发凭证？`}
          description="旧凭证立即作废，agent 掉线，必须用新命令重装。仅在凭证可能泄露时使用。"
          confirmLabel="换发凭证"
          busy={rotating}
          onClose={() => setConfirmRotate(false)}
          onConfirm={rotate}
        />
      )}
    </Dialog>
  )
}

// Traffic turns a subdued orange at the alert threshold and a subdued red once
// the allowance is used up, so the table agrees with the alerts. With alerts
// off, the threshold's default of 80 % still marks a node running short.
export function trafficTone(n: Node, warnAt: number) {
  if (n.traffic_limit <= 0) return ""
  if (n.month_used >= n.traffic_limit) return "text-over"
  return n.month_used * 100 >= n.traffic_limit * warnAt ? "text-near" : ""
}

export function Nodes({ nodes, refresh, site, refusal }: { nodes: Node[]; refresh: () => void; site: string; refusal: string }) {
  const warnAt = Number(useSettings().s?.notify_traffic) || 80
  const [creating, setCreating] = useState(false)
  const [editing, setEditing] = useState<Node | null>(null)
  const [billing, setBilling] = useState<Node | null>(null)
  const [installing, setInstalling] = useState<Node | null>(null)
  const [registering, setRegistering] = useState(false)
  const reg = useRegisterWindow()
  const [deleting, setDeleting] = useState<Node | null>(null)
  const [removing, setRemoving] = useState(false)
  const [query, setQuery] = useState("")
  const [group, setGroup] = useGroupFilter(nodes)
  const [grouping, setGrouping] = useState(false)
  // A new node lands last, a hundred rows down on a large fleet, so it is
  // brought into view once the list carries it. A filter hiding it cancels the
  // scroll rather than leaving one to fire when the filter is cleared.
  const added = useRef<number | null>(null)
  const drag = useDragOrder(nodes, "nodes", refresh)
  const visible = inGroup(searchNodes(drag.order, query), group)
  useEffect(() => {
    if (added.current === null || !nodes.some((n) => n.id === added.current)) return
    document.querySelector(`tbody tr[data-id="${added.current}"]`)?.scrollIntoView({ block: "center", behavior: "smooth" })
    added.current = null
  })
  const searching = query.trim() !== "" || group !== "all"
  const uninstall = refusal ? "" : uninstallCommand(site)

  async function remove() {
    if (!deleting) return
    setRemoving(true)
    try {
      await api(`/nodes/${deleting.id}`, { method: "DELETE" })
      toast.success("已删除")
      setDeleting(null)
      refresh()
    } catch (e) {
      toast.error((e as Error).message)
    } finally {
      setRemoving(false)
    }
  }

  return (
    <div className="space-y-4">
      {refusal && <p className="text-sm text-muted-foreground">{refusal}</p>}
      <div className="flex flex-wrap items-center justify-end gap-2">
        <div className="mr-auto flex w-full gap-2 sm:w-auto">
          <AdminSearchInput className="min-w-0 flex-1 sm:w-64 sm:flex-none" ariaLabel="搜索节点" placeholder="名称/地址/地区/分组" value={query} onChange={setQuery} />
          <GroupFilter nodes={nodes} value={group} onChange={setGroup} className="w-32" />
        </div>
        <Button variant="outline" disabled={!nodes.length} onClick={() => setGrouping(true)}>
          <Layers /> 分组
        </Button>
        {/* An open window is visible from the list itself, so nobody has to
            remember they left one open. */}
        <Button variant="outline" disabled={!!refusal} onClick={() => { reg.sync(); setRegistering(true) }}>
          <Server /> 批量添加{reg.left > 0 && ` · ${Math.ceil(reg.left / 60)} 分`}
        </Button>
        <Button disabled={!!refusal} onClick={() => setCreating(true)}>
          <Plus /> 添加节点
        </Button>
      </div>

      <Card className="overflow-x-auto p-0">
        <Table>
          <TableHeader>
            {/* Percentages, or the address column swallows every spare pixel
                and pushes status across the table. */}
            <TableRow>
              <TableHead className="w-[26%]">名称</TableHead>
              <TableHead className="w-[18%]">IP</TableHead>
              <TableHead className="w-[12%]">状态</TableHead>
              <TableHead className="w-[16%]">流量</TableHead>
              {/* Below xl the expiry date moves under the price: seven
                  columns leave a 1024px window no room for names. */}
              <TableHead className="w-[10%]">价格<span className="xl:hidden"> / 到期</span></TableHead>
              <TableHead className="hidden w-[12%] xl:table-cell">到期</TableHead>
              <TableHead className="text-right">操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {visible.map((n) => (
              <TableRow key={n.id} {...drag.row(n.id)}>
                {/* Wraps: under the cell's default nowrap, one long name would
                    widen the table until the actions left the screen. */}
                <TableCell className="whitespace-normal">
                  <div className="flex items-center gap-2">
                    <DragHandle
                      {...drag.handle(n.id)}
                      name={n.name}
                      disabled={searching}
                      title={searching ? "清空搜索和分组筛选后可拖动排序" : undefined}
                    />
                    <div className="min-w-24">
                      {/* Lines are balanced, so none ends on a character or two. */}
                      <div className="font-medium text-balance break-keep">
                        {nameText(n.name)}
                        {/* In the name's flow, a fixed gap after its last word.
                            Beside the block it would sit at the cell's edge
                            whenever the name or group wraps, since a wrapped
                            block spans the whole width. The gap is a figure
                            space, which does not break: the badge moves down
                            with the last word rather than open a line alone. */}
                        {n.country && "\u2007"}
                        {n.country && (
                          <Badge
                            variant="outline"
                            title={n.country_pin ? "手动指定" : undefined}
                            className="align-middle font-normal text-muted-foreground"
                          >
                            {n.country}
                          </Badge>
                        )}
                      </div>
                      {n.group && <div className="text-xs text-balance text-muted-foreground">{n.group}</div>}
                    </div>
                  </div>
                </TableCell>
                {/* Addresses live only here, never on the public page. */}
                <TableCell>
                  <Addresses list={n.addresses ?? []} />
                </TableCell>
                <TableCell>
                  {/* Stacked and centred on one axis. The slot is as wide as
                      the three-character 不公开, so a lone pill sits where it
                      would above that one and every row lines up. */}
                  <div className="flex w-fit min-w-14 flex-col items-center gap-1">
                    <Badge variant={n.online ? "default" : "secondary"} className="font-normal">
                      {n.online ? "在线" : "离线"}
                    </Badge>
                    {!n.public && <Badge variant="outline" className="font-normal">不公开</Badge>}
                    {/* Under the badge, not inside it: the column is a tenth of
                        the table and the three do not share one line. A slot of
                        the pills' width that the text spills out of evenly, so a
                        long duration does not widen the slot and move the
                        pills off the axis the other rows share. */}
                    {!n.online && n.last_seen_ago !== null && n.last_seen_ago >= 60 && (
                      <div className="flex w-14 justify-center">
                        <span className="tnum text-xs whitespace-nowrap text-muted-foreground">
                          {uptime(n.last_seen_ago)}
                        </span>
                      </div>
                    )}
                  </div>
                </TableCell>
                {/* Counted by the node's own billing rule, as on the public
                    page. Two unbreakable halves, so a narrow table moves the
                    limit to a second line rather than splitting a figure. */}
                <TableCell className="tnum text-sm whitespace-normal">
                  <span className={`whitespace-nowrap ${trafficTone(n, warnAt)}`}>
                    {bytes(n.month_used)}
                  </span>{" "}
                  <span className="whitespace-nowrap text-muted-foreground">
                    / {n.traffic_limit > 0 ? bytes(n.traffic_limit) : FOREVER}
                  </span>
                </TableCell>
                <TableCell className="tnum text-sm">
                  {n.price > 0 ? money(n.price, n.currency) : "免费"}
                  <div className="text-xs text-muted-foreground xl:hidden">{n.expires_at || FOREVER}</div>
                </TableCell>
                <TableCell className="hidden text-sm xl:table-cell">{n.expires_at || FOREVER}</TableCell>
                <TableCell className="text-right whitespace-nowrap">
                  <Button variant="ghost" size="icon" disabled={!!refusal} onClick={() => setInstalling(n)} title="安装 Agent" aria-label="安装 Agent">
                    <Download />
                  </Button>
                  <Button variant="ghost" size="icon" onClick={() => setEditing(n)} title="编辑节点" aria-label="编辑节点">
                    <Pencil />
                  </Button>
                  <Button variant="ghost" size="icon" onClick={() => setBilling(n)} title="续费设置" aria-label="续费设置">
                    <CalendarClock />
                  </Button>
                  <Button variant="ghost" size="icon" onClick={() => setDeleting(n)} title="删除节点" aria-label="删除节点">
                    <Trash2 className="text-destructive" />
                  </Button>
                </TableCell>
              </TableRow>
            ))}
            {nodes.length === 0 && (
              <TableRow>
                <TableCell colSpan={7} className="py-10 text-center text-sm text-muted-foreground">
                  还没有节点，右上角添加
                </TableCell>
              </TableRow>
            )}
            {searching && nodes.length > 0 && !visible.length && (
              <TableRow>
                <TableCell colSpan={7} className="py-10 text-center text-sm text-muted-foreground">
                  没有匹配的节点
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </Table>
      </Card>

      {creating && (
        <CreateNode
          onClose={() => setCreating(false)}
          onSaved={(id) => { added.current = id; refresh() }}
        />
      )}
      {editing && (
        <NodeForm
          node={editing}
          nodes={nodes}
          onClose={() => setEditing(null)}
          onSaved={refresh}
        />
      )}
      {grouping && <GroupDialog nodes={drag.order} onClose={() => setGrouping(false)} onSaved={refresh} />}
      {billing && (
        <BillingForm node={billing} onClose={() => setBilling(null)} onSaved={refresh} />
      )}
      {registering && <RegisterDialog site={site} reg={reg} onClose={() => { setRegistering(false); refresh() }} />}

      {installing && (
        <InstallDialog
          node={installing}
          site={site}
          onClose={() => setInstalling(null)}
          onRotated={refresh}
        />
      )}
      {deleting && (
        <ConfirmDialog
          title={`删除节点「${deleting.name}」？`}
          description="历史指标、流量记录和凭证一并删除，不可恢复。"
          confirmLabel="删除节点"
          busy={removing}
          onClose={() => setDeleting(null)}
          onConfirm={remove}
        >
          {/* Deleting the node leaves the agent running on the machine, retrying
              with a token the hub no longer accepts. */}
          {uninstall && (
            <div className="space-y-2">
              <div className="flex items-center justify-between gap-2">
                <Label className="text-sm font-medium">卸载 agent</Label>
                <Button variant="ghost" size="sm" onClick={() => copy(uninstall)}>
                  <Copy className="size-4" /> 复制
                </Button>
              </div>
              <Command>{uninstall}</Command>
              <p className="text-xs text-muted-foreground">
                在这台机器上以 root 执行，停止 agent，删除二进制、env 文件和服务文件。
              </p>
            </div>
          )}
        </ConfirmDialog>
      )}
    </div>
  )
}
