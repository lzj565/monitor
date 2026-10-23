import type { Node } from "./api.ts"

export type NodeFrame = { nodes: Node[] | null; admin: boolean; stale?: boolean; error?: string }
type Socket = Pick<WebSocket, "onmessage" | "onclose" | "onerror" | "close">
type Visibility = { hidden: () => boolean; subscribe: (listener: () => void) => () => void }
type Options = {
  fetchNodes: (signal: AbortSignal) => Promise<NodeFrame>
  openSocket: () => Socket
  onFrame: (frame: NodeFrame) => void
  onError: (error: Error) => void
  delay?: number
  silence?: number
  visibility?: Visibility
}

/** Pause hidden pages and replace every old request/socket when resuming. */
export function createNodeStream(options: Options) {
  const visibility = options.visibility ?? (typeof document === "undefined" ? undefined : {
    hidden: () => document.hidden,
    subscribe: (listener: () => void) => {
      document.addEventListener("visibilitychange", listener)
      return () => document.removeEventListener("visibilitychange", listener)
    },
  })
  let stop: (() => void) | undefined
  let disposed = false
  const sync = () => {
    stop?.()
    stop = undefined
    if (!disposed && !visibility?.hidden()) stop = activeStream(options)
  }
  const unsubscribe = visibility?.subscribe(sync)
  sync()
  return () => { disposed = true; unsubscribe?.(); stop?.(); stop = undefined }
}

/** One HTTP request at a time; a stream frame invalidates older HTTP replies. */
function activeStream({ fetchNodes, openSocket, onFrame, onError, delay = 5000, silence = 10_000 }: Options) {
  const controller = new AbortController()
  let socket: Socket | null = null
  let poll: ReturnType<typeof setTimeout> | undefined
  let retry: ReturnType<typeof setTimeout> | undefined
  let watchdog: ReturnType<typeof setTimeout> | undefined
  let pending = false
  let streaming = false
  let revision = 0
  const active = () => !controller.signal.aborted
  const schedulePoll = () => {
    if (active() && !streaming && !pending && poll === undefined) {
      poll = setTimeout(() => { poll = undefined; void fetchOnce() }, delay)
    }
  }
  const fetchOnce = async () => {
    if (!active() || pending) return
    pending = true
    const started = revision
    try {
      const frame = await fetchNodes(controller.signal)
      if (active() && revision === started) onFrame(frame)
    } catch (error) {
      if (active() && revision === started) onError(error as Error)
    } finally {
      pending = false
      schedulePoll()
    }
  }
  const detach = (current: Socket) => {
    current.onmessage = current.onclose = current.onerror = null
    if (socket === current) socket = null
    if (watchdog !== undefined) clearTimeout(watchdog)
    watchdog = undefined
    current.close()
  }
  const disconnected = (current: Socket, immediate = false) => {
    if (!active() || socket !== current) return
    // Do not await the close handshake: a dead connection can take a minute.
    detach(current)
    streaming = false
    if (immediate) void fetchOnce()
    else schedulePoll()
    if (retry !== undefined) clearTimeout(retry)
    retry = setTimeout(() => { retry = undefined; connect() }, immediate ? 0 : delay)
  }
  const connect = () => {
    if (!active()) return
    try {
      const current = openSocket()
      socket = current
      const watch = () => {
        if (watchdog !== undefined) clearTimeout(watchdog)
        watchdog = setTimeout(() => {
          if (!active() || socket !== current) return
          onError(new Error("实时数据中断，正在重新连接"))
          disconnected(current, true)
        }, silence)
      }
      watch()
      current.onmessage = (event) => {
        if (!active() || socket !== current) return
        let frame: NodeFrame
        try {
          frame = JSON.parse(String(event.data))
          if (!frame || (!Array.isArray(frame.nodes) && frame.nodes !== null) || typeof frame.admin !== "boolean") {
            throw new Error("实时数据格式错误")
          }
        } catch {
          onError(new Error("实时数据格式错误，正在重新连接"))
          disconnected(current)
          return
        }
        watch()
        revision += 1
        streaming = true
        if (poll !== undefined) clearTimeout(poll)
        poll = undefined
        onFrame(frame)
      }
      current.onerror = () => disconnected(current)
      current.onclose = () => disconnected(current)
    } catch {
      streaming = false
      schedulePoll()
      retry = setTimeout(() => { retry = undefined; connect() }, delay)
    }
  }
  void fetchOnce()
  connect()
  return () => {
    controller.abort()
    if (poll !== undefined) clearTimeout(poll)
    if (retry !== undefined) clearTimeout(retry)
    if (watchdog !== undefined) clearTimeout(watchdog)
    if (socket) detach(socket)
  }
}
