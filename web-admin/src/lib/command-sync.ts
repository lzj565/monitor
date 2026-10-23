import type { SyncReport, SyncResolution } from "./resources.ts"

type Command = { status: string; error?: { message?: string } }

function pause(ms: number, signal: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    const done = () => { signal.removeEventListener("abort", aborted); resolve() }
    const timer = setTimeout(done, ms)
    const aborted = () => { clearTimeout(timer); reject(signal.reason) }
    if (signal.aborted) aborted()
    else signal.addEventListener("abort", aborted, { once: true })
  })
}

/** A shared deadline and bounded workers keep fleet size from multiplying load. */
export async function resolveCommandSync(
  report: SyncReport,
  fetchCommand: (nodeId: number, commandId: string, signal: AbortSignal) => Promise<Command>,
  options: { signal?: AbortSignal; timeout?: number; interval?: number } = {},
): Promise<SyncResolution> {
  const resolution: SyncResolution = { succeeded: [], failed: [], pending: [] }
  const deadline = Date.now() + (options.timeout ?? 36_000)
  const signal = options.signal ?? new AbortController().signal
  let next = 0
  const worker = async () => {
    while (next < report.queued.length) {
      signal.throwIfAborted()
      const { node_id, command_id } = report.queued[next++]
      let delay = options.interval ?? 500
      let finished = false
      while (Date.now() < deadline) {
        signal.throwIfAborted()
        const requestController = new AbortController()
        const requestSignal = AbortSignal.any([signal, requestController.signal])
        let rejectRequest!: (reason: unknown) => void
        const aborted = () => rejectRequest(requestSignal.reason)
        const timer = setTimeout(() => requestController.abort(new DOMException("查询超时", "TimeoutError")), Math.max(1, Math.min(5000, deadline - Date.now())))
        try {
          const cancellation = new Promise<never>((_, reject) => { rejectRequest = reject })
          requestSignal.addEventListener("abort", aborted, { once: true })
          requestSignal.throwIfAborted()
          const command = await Promise.race([fetchCommand(node_id, command_id, requestSignal), cancellation])
          if (command.status === "succeeded") {
            resolution.succeeded.push(node_id)
            finished = true
            break
          }
          if (command.status === "failed") {
            resolution.failed.push({ node_id, message: command.error?.message ?? "Agent 应用失败" })
            finished = true
            break
          }
        } catch (error) {
          signal.throwIfAborted()
          const status = (error as { status?: number } | null)?.status
          if (status !== undefined && status > 0 && status < 500 && status !== 429) {
            resolution.failed.push({ node_id, message: error instanceof Error ? error.message : "查询命令结果失败" })
            finished = true
            break
          }
        }
        finally {
          clearTimeout(timer)
          requestSignal.removeEventListener("abort", aborted)
        }
        const remaining = deadline - Date.now()
        if (remaining > 0) await pause(Math.min(delay, remaining), signal)
        delay = Math.min(delay * 1.5, 3000)
      }
      if (!finished) resolution.pending.push(node_id)
    }
  }
  await Promise.all(Array.from({ length: Math.min(4, report.queued.length) }, worker))
  return resolution
}
