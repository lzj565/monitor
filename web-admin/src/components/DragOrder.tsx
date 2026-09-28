import * as React from "react"
import { useEffect, useRef, useState } from "react"
import { flushSync } from "react-dom"
import { GripVertical } from "lucide-react"
import { toast } from "sonner"

import { api } from "@/lib/api"

// Reordering uses the browser's view transitions, so displaced rows slide.
// Browsers without support jump instead. A drag starts a transition on every row
// it crosses, each skipping the last, and a skipped transition rejects `ready`.
function animate(update: () => void) {
  if (document.startViewTransition) document.startViewTransition(() => flushSync(update)).ready.catch(() => {})
  else flushSync(update)
}

// Drag-to-reorder for a table whose order the hub stores at `/${path}/order`.
// Rows are displaced while the pointer is down and the whole order is saved on
// release, so a filtered table must disable its handles: the rows on screen are
// then not `order`.
export function useDragOrder<T extends { id: number }>(items: T[], path: string, reload: () => void) {
  const [manualOrder, setManualOrder] = useState<number[]>([])
  const [dragging, setDragging] = useState<number | null>(null)
  const orderBeforeDrag = useRef<number[]>([])
  // The order last asked for. A view transition renders it a frame or more
  // later, and a repeated key or a quick drag must build on it rather than on
  // the order still on screen.
  const pending = useRef<number[] | null>(null)
  const body = useRef<HTMLTableSectionElement | null>(null)
  // One save in flight at a time, so two quick reorders reach the hub in order.
  const saving = useRef<Promise<unknown>>(Promise.resolve())
  const byId = new Map(items.map((item) => [item.id, item]))
  const orderedIds = new Set(manualOrder)
  const order = [
    ...manualOrder.map((id) => byId.get(id)).filter((item): item is T => Boolean(item)),
    ...items.filter((item) => !orderedIds.has(item.id)),
  ]
  const ids = () => pending.current ?? order.map((item) => item.id)

  // Once the hub lists this order, its list is followed again, so a reorder made
  // in another tab appears here instead of being overwritten by the next drag.
  if (dragging === null && manualOrder.length && manualOrder.join() === items.map((item) => item.id).join()) {
    setManualOrder([])
  }

  // Handled on the document by where the pointer is, not by the row under it:
  // while a view transition runs, Chrome hit-tests drag events to the root
  // element, so row handlers would miss every row crossed during a 150 ms slide,
  // and a release then would count as a drop outside the table. Re-attached on
  // every render, since `order` changes as rows are displaced.
  useEffect(() => {
    const rows = body.current
    if (dragging === null || !rows) return
    const over = (e: DragEvent) => {
      const table = rows.getBoundingClientRect()
      if (e.clientX < table.left || e.clientX > table.right || e.clientY < table.top || e.clientY > table.bottom) return
      e.preventDefault()
      if (e.type === "drop") return
      if (e.dataTransfer) e.dataTransfer.dropEffect = "move"
      const at = [...rows.rows].findIndex((row) => {
        const r = row.getBoundingClientRect()
        return e.clientY >= r.top && e.clientY < r.bottom
      })
      // By position rather than by the row there, which may be one a pending
      // transition has yet to move.
      if (at >= 0) move(dragging, at)
    }
    document.addEventListener("dragover", over)
    document.addEventListener("drop", over)
    return () => {
      document.removeEventListener("dragover", over)
      document.removeEventListener("drop", over)
    }
  })

  // A transition superseded before it ran applies nothing: a later move, or a
  // refused save falling back to the hub's order, has replaced it.
  function show(next: number[]) {
    pending.current = next
    animate(() => {
      if (pending.current !== next) return
      pending.current = null
      setManualOrder(next)
    })
  }

  function move(id: number, to: number) {
    const next = [...ids()]
    const from = next.indexOf(id)
    if (from < 0 || to < 0 || to >= next.length || from === to) return
    next.splice(to, 0, ...next.splice(from, 1))
    show(next)
    return next
  }

  // Dropped outside the table or cancelled with Escape: the order is restored.
  function cancel() {
    setDragging(null)
    if (orderBeforeDrag.current.length) show(orderBeforeDrag.current)
  }

  function save(next: number[]) {
    setDragging(null)
    const before = orderBeforeDrag.current
    if (!before.length || next.join() === before.join()) return
    orderBeforeDrag.current = next
    const put = () => api(`/${path}/order`, { method: "PUT", body: JSON.stringify({ ids: next }) })
    // A refusal falls back to whatever the hub holds, which a save queued
    // behind it may still change.
    saving.current = saving.current.then(put).then(reload, (e: Error) => {
      pending.current = null
      setManualOrder([])
      reload()
      toast.error(e.message)
    })
  }

  return {
    order,
    row: (id: number) => ({
      style: { viewTransitionName: `${path}-${id}` },
      "data-dragging": dragging === id || undefined,
      className: "transition-opacity data-[dragging]:opacity-40",
    }),
    handle: (id: number) => ({
      onDragStart: (e: React.DragEvent<HTMLElement>) => {
        orderBeforeDrag.current = ids()
        body.current = e.currentTarget.closest("tbody")
        setDragging(id)
        e.dataTransfer.effectAllowed = "move"
        // Firefox refuses to start a drag without a payload.
        e.dataTransfer.setData("text/plain", String(id))
      },
      onDragEnd: (e: React.DragEvent) => (e.dataTransfer.dropEffect === "none" ? cancel() : save(ids())),
      onKeyDown: (e: React.KeyboardEvent) => {
        const delta = e.key === "ArrowUp" ? -1 : e.key === "ArrowDown" ? 1 : 0
        if (!delta) return
        e.preventDefault()
        orderBeforeDrag.current = ids()
        const next = move(id, ids().indexOf(id) + delta)
        if (next) save(next)
      },
    }),
  }
}

export function DragHandle({ name, disabled, title = "拖动排序", ...events }: React.ComponentProps<"button"> & { name: string }) {
  return (
    <button
      type="button"
      draggable={!disabled}
      disabled={disabled}
      className="cursor-grab touch-none rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground active:cursor-grabbing disabled:cursor-default disabled:opacity-40 disabled:hover:bg-transparent"
      title={title}
      aria-label={`拖动 ${name} 排序`}
      {...events}
    >
      <GripVertical className="size-4" />
    </button>
  )
}

