import { memo, useEffect, useId, useRef, useState } from "react"
import { Check, ChevronDown, CircleQuestionMark, Copy, Search } from "lucide-react"
import { toast } from "sonner"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Popover, PopoverAnchor, PopoverContent } from "@/components/ui/popover"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip"
import { groupsOf, inGroup, shortAddress, type Node, type Source, type PingTask } from "@/lib/api"

// Counters the panel can correct after migration or an accounting error.
export const TRAFFIC_FIELDS = [
  ["total_rx", "累计下行"],
  ["total_tx", "累计上行"],
  ["month_rx", "本月下行"],
  ["month_tx", "本月上行"],
] as const
export const TRAFFIC_MODES: Record<string, string> = {
  sum: "上下行相加",
  max: "取较大值",
  up: "仅上行",
  down: "仅下行",
}

export function copy(text: string, done = "已复制") {
  // navigator.clipboard exists only in a secure context. Over plain http the
  // copy command still works from a click, the clipboard filled from its event.
  if (!navigator.clipboard) {
    const put = (e: ClipboardEvent) => { e.clipboardData?.setData("text/plain", text); e.preventDefault() }
    document.addEventListener("copy", put)
    const ok = document.execCommand("copy")
    document.removeEventListener("copy", put)
    return ok ? toast.success(done) : toast.error("复制失败")
  }
  navigator.clipboard.writeText(text).then(
    () => toast.success(done),
    () => toast.error("复制失败"),
  )
}

export const SOURCES: Record<Source, string> = {
  manual: "手动填写",
  interface: "网卡地址",
  exit: "hub 看到的出口，不在节点网卡上（NAT 或代理）",
  connection: "hub 看到的连接地址",
}

// The address a node is reached by, one per family, each click-to-copy: pasting
// one into an ssh command is why they are shown. Where each came from is in the
// tooltip, keeping the column to addresses alone.
//
// Drawn again only when the addresses change. The table re-renders on every
// push, and redrawing a tooltip per address would raise the panel's script time
// at a hundred nodes from 66 to 128 ms a second.
export const Addresses = memo(
  function Addresses({ list }: { list: NonNullable<Node["addresses"]> }) {
    if (!list.length) return <span className="text-sm text-muted-foreground">—</span>
    return (
      // As wide as the longer address, so both tooltips open from one right
      // edge and the one for a short IPv4 does not cover the IPv6 below it.
      <div className="grid w-fit gap-y-0.5">
        {list.map(({ address, source }) => (
          // Beside the addresses rather than under one, where it would cover
          // the other; and gone once the pointer leaves it.
          <Tooltip key={address} disableHoverableContent>
            <TooltipTrigger asChild>
              <button
                type="button"
                // The toast names what was copied: a tap shows no tooltip, and
                // the cell may show the address shortened.
                onClick={() => copy(address, `已复制 ${address}`)}
                aria-label={`复制 ${address}`}
                className="tnum group inline-flex items-center gap-1 text-sm hover:text-foreground"
              >
                {shortAddress(address)}
                <Copy className="size-3 shrink-0 opacity-0 transition-opacity group-hover:opacity-100" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right" sideOffset={6} className="max-w-xs">
              <div className="tnum">{address}</div>
              <div className="opacity-70">{SOURCES[source]}，点击复制</div>
            </TooltipContent>
          </Tooltip>
        ))}
      </div>
    )
  },
  // Rows arrive as fresh objects on every push, so the list is compared by value.
  (a, b) => JSON.stringify(a.list) === JSON.stringify(b.list),
)

// Name, address, country and group: what a node is looked up by, in every node list.
export function searchNodes(nodes: Node[], query: string) {
  const needle = query.trim().toLowerCase()
  if (!needle) return nodes
  return nodes.filter((n) =>
    [n.name, n.ip, n.ipv4, n.ipv6, n.ipv4_pin, n.ipv6_pin, n.country, n.group].some((v) => v?.toLowerCase().includes(needle)))
}

// A filter naming a group no node carries any more -- renamed, or its last node
// deleted -- falls back to all rather than showing an empty list; so does 未分组
// once no group is left, since the dropdown that would clear it is hidden then.
// Reset rather than masked, so the old filter does not return with a later group
// of the same name.
export function useGroupFilter(nodes: Node[]) {
  const [filter, setFilter] = useState("all")
  const valid = filter === "all"
    || (filter === "none" ? nodes.some((n) => n.group) : nodes.some((n) => n.group === filter.slice(1)))
  if (!valid) setFilter("all")
  return [valid ? filter : "all", setFilter] as const
}

// Offered once some node has a group. 未分组 is where a batch of freshly
// registered machines waits to be assigned one.
export function GroupFilter({ nodes, value, onChange, className = "" }: {
  nodes: Node[]
  value: string
  onChange: (value: string) => void
  className?: string
}) {
  const groups = groupsOf(nodes)
  if (!groups.length) return null
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger className={className} aria-label="按分组筛选"><SelectValue /></SelectTrigger>
      <SelectContent position="popper">
        <SelectItem value="all">全部分组</SelectItem>
        {groups.map((g) => <SelectItem key={g} value={`=${g}`}>{g}</SelectItem>)}
        <SelectItem value="none">未分组</SelectItem>
      </SelectContent>
    </Select>
  )
}

// Ticks nodes in a searchable grid. 全选 and 全不选 act on the rows in view, so a
// search or a group narrows what they touch: pick a group, then 全选. Offline
// nodes are dimmed but remain selectable.
export function NodePicker({ nodes, chosen, onPick, disabled = false }: {
  nodes: Node[]
  chosen: Set<number>
  onPick: (list: Node[], on: boolean) => void
  disabled?: boolean
}) {
  const [query, setQuery] = useState("")
  const [group, setGroup] = useGroupFilter(nodes)
  return (
    <Picker
      visible={inGroup(searchNodes(nodes, query), group)}
      chosen={chosen}
      onPick={onPick}
      disabled={disabled}
      toolbar={<>
        <NodeSearch className="min-w-0 flex-1 basis-40" value={query} onChange={setQuery} />
        <GroupFilter nodes={nodes} value={group} onChange={setGroup} className="w-32" />
      </>}
      empty={nodes.length ? "没有匹配的节点" : "先添加节点"}
      // Three columns keep a few dozen nodes within one scroll. A phone gets
      // one: two cut a name to a few characters, and a tap shows no title.
      listClass="grid-cols-1 min-[480px]:grid-cols-2 sm:grid-cols-3"
      title={(n) => (n.group ? `${n.name} · ${n.group}` : n.name)}
    >
      {(n) => <>
        <span className={`truncate ${n.online ? "" : "text-muted-foreground"}`}>{n.name}</span>
        {n.country && <span className="ml-auto shrink-0 text-xs text-muted-foreground">{n.country}</span>}
      </>}
    </Picker>
  )
}

// Every command the panel hands out. break-all because a token has no spaces to
// wrap at.
export function Command({ className = "", children }: { className?: string; children: React.ReactNode }) {
  return (
    <pre className={`overflow-auto whitespace-pre-wrap break-all rounded-lg border bg-muted/40 p-3 text-xs leading-relaxed select-all ${className}`}>
      {children}
    </pre>
  )
}

export function Field({ label, hint, help, helpWidth, className = "", children }: {
  label: string
  hint?: string
  help?: React.ReactNode
  helpWidth?: string
  className?: string
  children: React.ReactNode
}) {
  const title = <Label className="text-sm font-medium">{label}</Label>
  return (
    <div className={`space-y-2 ${className}`}>
      {help ? <div className="flex items-center gap-1.5">{title}<Help width={helpWidth}>{help}</Help></div> : title}
      {children}
      {/* Kept whole between punctuation, as in Help below. */}
      {hint && <p className="text-xs leading-relaxed text-muted-foreground break-keep wrap-anywhere">{hint}</p>}
    </div>
  )
}

// A tap shows no tooltip on its own, so a click opens it as well. The trigger's
// own handlers would close it on press and on click; both are prevented.
export function Help({ children, width = "max-w-64" }: { children: React.ReactNode; width?: string }) {
  const [open, setOpen] = useState(false)
  return (
    <Tooltip open={open} onOpenChange={setOpen}>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label="说明"
          className="text-muted-foreground hover:text-foreground"
          onPointerDown={(e) => e.preventDefault()}
          onClick={(e) => {
            e.preventDefault()
            setOpen(true)
          }}
        >
          <CircleQuestionMark className="size-3.5" />
        </button>
      </TooltipTrigger>
      {/* text-wrap over the component's text-balance, which breaks multi-line
          Chinese halfway across the box. Chinese may also break between any
          two characters, which splits words such as 季付 across lines; kept
          whole, a line breaks at punctuation and spaces, and mid-run only when
          a run cannot fit at all. */}
      <TooltipContent
        collisionPadding={16}
        className={`${width} space-y-1 text-left text-wrap break-keep wrap-anywhere`}
      >
        {children}
      </TooltipContent>
    </Tooltip>
  )
}

// A titled option with its control at the right. `toggle` makes the whole row a
// label, so a click anywhere flips the Switch it holds; `below` opens beneath it
// in the same card.
export function OptionRow({ title, hint, toggle = false, below, children }: {
  title: React.ReactNode
  hint?: React.ReactNode
  toggle?: boolean
  below?: React.ReactNode
  children: React.ReactNode
}) {
  const Row = toggle ? "label" : "div"
  return (
    <div className="flex flex-col rounded-lg border bg-muted/30 text-sm">
      {/* flex-1: stretched by a grid, the row fills the card and stays clickable. */}
      <Row className={`flex flex-1 items-center justify-between gap-4 px-3 py-2.5 ${toggle ? "cursor-pointer" : ""}`}>
        <span>
          <span className="block font-medium">{title}</span>
          {hint && <span className="mt-0.5 block text-xs text-muted-foreground">{hint}</span>}
        </span>
        {children}
      </Row>
      {below && <div className="border-t px-3 pt-3 pb-3.5">{below}</div>}
    </div>
  )
}


// Free text, with the groups already in use offered, so a group is picked
// rather than retyped, where a typo would start a second one. A list of its own
// rather than a <datalist>: Chrome draws that as a tooltip and filters it by the
// text already in the box, so a grouped node was offered only its own group.
// The whole list shows on opening; typing narrows it without highlighting, so
// Enter keeps a new name that merely prefixes an existing one.
export function GroupInput({ nodes, value, onChange }: {
  nodes: Pick<Node, "group">[]
  value: string
  onChange: (value: string) => void
}) {
  const id = useId()
  const [open, setOpen] = useState(false)
  const [typed, setTyped] = useState(false)
  const [active, setActive] = useState(0)
  const input = useRef<HTMLInputElement>(null)
  const counts = new Map<string, number>()
  for (const n of nodes) if (n.group) counts.set(n.group, (counts.get(n.group) ?? 0) + 1)
  const name = value.trim()
  const query = typed ? name.toLowerCase() : ""
  const matches = [...counts.keys()].filter((g) => g.toLowerCase().includes(query))
  // "" is 未分组, offered last while the list is not being narrowed.
  const items = query ? matches : [...matches, ""]
  const fresh = typed && name !== "" && !counts.has(name)
  const shown = open && (matches.length > 0 || fresh)

  useEffect(() => {
    if (shown) document.getElementById(`${id}-${active}`)?.scrollIntoView({ block: "nearest" })
  }, [id, active, shown])

  const show = () => {
    setTyped(false)
    setActive(Math.max(0, [...counts.keys(), ""].indexOf(name)))
    setOpen(true)
  }
  const pick = (group: string) => {
    onChange(group)
    setTyped(false)
    setOpen(false)
  }
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault()
      if (!shown) return show()
      if (!items.length) return
      const down = e.key === "ArrowDown"
      setActive((i) => (i < 0 ? (down ? 0 : items.length - 1) : (i + (down ? 1 : -1) + items.length) % items.length))
    } else if (e.key === "Enter" && shown) {
      e.preventDefault()
      if (items[active] !== undefined) pick(items[active])
      else setOpen(false)
    }
  }

  if (!counts.size) {
    return <Input maxLength={13} value={value} onChange={(e) => onChange(e.target.value)} placeholder="未分组" />
  }
  return (
    <Popover open={shown} onOpenChange={setOpen}>
      <PopoverAnchor asChild>
        <div className="relative">
          <Input
            ref={input}
            role="combobox"
            aria-expanded={shown}
            aria-controls={id}
            aria-autocomplete="list"
            aria-activedescendant={shown && items[active] !== undefined ? `${id}-${active}` : undefined}
            maxLength={13}
            value={value}
            placeholder="未分组"
            className="pr-9"
            onChange={(e) => {
              onChange(e.target.value)
              setTyped(true)
              setActive(-1)
              setOpen(true)
            }}
            onClick={show}
            onKeyDown={onKeyDown}
          />
          {/* Not a tab stop, and keeps the focus in the box it opens a list for. */}
          <button
            type="button"
            tabIndex={-1}
            aria-label="选择分组"
            className="absolute inset-y-0 right-0 flex w-9 items-center justify-center text-muted-foreground"
            onMouseDown={(e) => {
              e.preventDefault()
              if (shown) return setOpen(false)
              input.current?.focus()
              show()
            }}
          >
            <ChevronDown className={`size-4 opacity-50 transition-transform ${shown ? "rotate-180" : ""}`} />
          </button>
        </div>
      </PopoverAnchor>
      <PopoverContent
        align="start"
        className="w-(--radix-popover-trigger-width) p-1"
        onOpenAutoFocus={(e) => e.preventDefault()}
        onCloseAutoFocus={(e) => e.preventDefault()}
        // The box and its button sit outside the list; pressing them is not a dismissal.
        onInteractOutside={(e) => input.current?.parentElement?.contains(e.target as Element) && e.preventDefault()}
        onMouseDown={(e) => e.preventDefault()}
      >
        <div role="listbox" id={id} aria-label="已有分组" className="max-h-60 overflow-y-auto">
          {items.map((group, i) => (
            <div key={group || "\0"}>
              {group === "" && <div className="my-1 h-px bg-border" />}
              <div
                id={`${id}-${i}`}
                role="option"
                aria-selected={group === name}
                onMouseEnter={() => setActive(i)}
                onClick={() => pick(group)}
                className={`relative flex cursor-default items-center gap-2 rounded-sm py-1.5 pr-8 pl-2 text-sm select-none ${i === active ? "bg-accent text-accent-foreground" : ""}`}
              >
                <span className={`truncate ${group ? "" : "text-muted-foreground"}`}>{group || "未分组"}</span>
                {group && <span className="tnum ml-auto shrink-0 text-xs text-muted-foreground">{counts.get(group)} 台</span>}
                {group === name && <Check className="absolute right-2 size-4" />}
              </div>
            </div>
          ))}
          {fresh && <p className="px-2 py-1.5 text-xs text-muted-foreground">新分组「{name}」，保存后生效</p>}
        </div>
      </PopoverContent>
    </Popover>
  )
}

// Puts the nodes ticked here into one group, or, with the name left empty, out
// of any. Renaming or dissolving a group is the same act: filter the picker to
// it, tick all, then type the new name or clear it. One request, applied to all
// of them or none.


export function NodeSearch({ value, onChange, className = "", placeholder = "名称/地址/地区/分组", label = "搜索节点" }: {
  value: string
  onChange: (value: string) => void
  className?: string
  placeholder?: string
  label?: string
}) {
  return (
    <div className={`relative ${className}`}>
      <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
      <Input
        className="pl-8"
        placeholder={placeholder}
        aria-label={label}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        // Inside a dialog's form, Enter would otherwise save the dialog.
        onKeyDown={(e) => e.key === "Enter" && e.preventDefault()}
      />
    </div>
  )
}


function Picker<T extends { id: number }>({ visible, chosen, onPick, disabled = false, toolbar, empty, listClass, title, children }: {
  visible: T[]
  chosen: Set<number>
  onPick: (list: T[], on: boolean) => void
  disabled?: boolean
  toolbar: React.ReactNode
  empty: string
  listClass: string
  title: (item: T) => string
  children: (item: T) => React.ReactNode
}) {
  // The unfiltered list's height, held as its floor: in a centred dialog a
  // shrinking list would move the search box out from under the cursor.
  const [listHeight, setListHeight] = useState(0)
  const visibleChosen = visible.filter((n) => chosen.has(n.id)).length
  return (
    <div className="rounded-lg border">
      <div className="flex flex-wrap items-center gap-1 border-b p-2">
        {toolbar}
        <Button type="button" size="sm" variant="ghost" className="px-2.5" disabled={disabled || visibleChosen === visible.length} onClick={() => onPick(visible, true)}>全选</Button>
        <Button type="button" size="sm" variant="ghost" className="px-2.5" disabled={disabled || visibleChosen === 0} onClick={() => onPick(visible, false)}>全不选</Button>
      </div>
      <div
        ref={(el) => { if (el && !listHeight) setListHeight(el.offsetHeight) }}
        // Capped like the height itself, which a min-height would otherwise
        // override once the viewport shrinks.
        style={{ minHeight: listHeight ? `min(${listHeight}px, 16rem, 40dvh)` : undefined }}
        className={`grid max-h-[min(16rem,40dvh)] content-start gap-0.5 overflow-y-auto p-1.5 ${listClass}`}
      >
        {visible.map((item) => (
          <label key={item.id} title={title(item)} className="flex min-w-0 cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-muted">
            <input type="checkbox" checked={chosen.has(item.id)} disabled={disabled} onChange={(e) => onPick([item], e.target.checked)} className="shrink-0 accent-primary" />
            {children(item)}
          </label>
        ))}
        {!visible.length && <p className="col-span-full p-2 text-xs text-muted-foreground">{empty}</p>}
      </div>
    </div>
  )
}


export function ProbePicker({ tasks, chosen, onPick }: {
  tasks: PingTask[]
  chosen: Set<number>
  onPick: (list: PingTask[], on: boolean) => void
}) {
  const [query, setQuery] = useState("")
  const needle = query.trim().toLowerCase()
  return (
    <Picker
      visible={needle ? tasks.filter((t) => `${t.name} ${t.target}`.toLowerCase().includes(needle)) : tasks}
      chosen={chosen}
      onPick={onPick}
      toolbar={<NodeSearch className="min-w-0 flex-1 basis-32" value={query} onChange={setQuery} placeholder="名称/目标" label="搜索监控" />}
      empty={tasks.length ? "没有匹配的监控" : "还没有延迟监控，先点「添加监控」"}
      listClass="grid-cols-1"
      title={(t) => `${t.name} · ${t.target}`}
    >
      {(t) => <>
        <span className="min-w-0 flex-1 truncate">{t.name}</span>
        <span className="tnum max-w-[45%] shrink-0 truncate text-xs text-muted-foreground">{t.target}</span>
        <span className="tnum w-12 shrink-0 text-right text-xs text-muted-foreground">{t.interval}s</span>
      </>}
    </Picker>
  )
}


export function SwitchRow({ label, hint, extra, help, helpWidth, checked, onChange }: {
  label: string
  hint?: string
  extra?: React.ReactNode
  help?: React.ReactNode
  helpWidth?: string
  checked: boolean
  onChange: (on: boolean) => void
}) {
  const id = useId()
  return (
    <div className="flex items-start gap-2">
      <Switch aria-labelledby={id} checked={checked} onCheckedChange={onChange} className="mt-0.5" />
      <div className="space-y-1">
        <div className="text-sm leading-snug text-balance">
          <span id={id}>{label}</span>
          {help && (
            <span className="ml-1.5 inline-flex align-middle">
              <Help width={helpWidth}>{help}</Help>
            </span>
          )}
        </div>
        {hint && <p className="text-xs leading-relaxed text-muted-foreground break-keep wrap-anywhere">{hint}</p>}
        {extra}
      </div>
    </div>
  )
}

// Width in ems, near enough: a CJK character is one, anything else about half.
export const ems = (word: string) => [...word].reduce((n, c) => n + (c > "\u2e7f" ? 1 : 0.55), 0)

// A node name, broken at its spaces and after the dots of a hostname --
// registered nodes are named after theirs. A word up to eight ems stays whole,
// a city, a label or a hyphenated one such as GIA-E, where the browser would
// leave its last letter to open the next line. A wider word, as a name typed
// without spaces usually is, breaks between its CJK characters, and a run of
// letters only where nothing else fits: kept whole, it would set the column's
// minimum width and push the table past the screen. A separator such as the ·
// in 香港 09 · DMIT, or a dash, is held to the word before it, so no line opens
// with one.
export function nameText(name: string) {
  return name
    .replace(/ (?=[·|/｜—–-] )/g, "\u00a0")
    .split(/( )/)
    .map((word, i) => {
      const parts = word.split(".").flatMap((part, j) => (j ? [".", <wbr key={j} />, part] : [part]))
      if (ems(word) > 8) return <span key={i} className="wrap-anywhere [word-break:normal]">{parts}</span>
      return word.includes("-") ? <span key={i} className="whitespace-nowrap">{word}</span> : parts
    })
}
