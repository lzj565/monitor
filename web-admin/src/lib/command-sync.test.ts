import assert from "node:assert/strict"
import { setTimeout as sleep } from "node:timers/promises"
import { resolveCommandSync } from "./command-sync.ts"

const report = { queued: Array.from({ length: 12 }, (_, index) => ({ node_id: index, command_id: String(index) })), needs_sync: [] }
let active = 0
let maximum = 0
const attempts = new Map<number, number>()
const result = await resolveCommandSync(report, async (nodeId) => {
  maximum = Math.max(maximum, ++active)
  await sleep(2)
  active -= 1
  const attempt = (attempts.get(nodeId) ?? 0) + 1
  attempts.set(nodeId, attempt)
  if (nodeId === 0) return { status: "failed", error: { message: "invalid config" } }
  if (nodeId === 1 && attempt === 1) throw Object.assign(new Error("temporary failure"), { status: 503 })
  if (nodeId === 2 && attempt === 1) return { status: "pending" }
  return { status: "succeeded" }
}, { timeout: 1000, interval: 1 })
assert.ok(maximum <= 4)
assert.equal(result.succeeded.length, 11)
assert.deepEqual(result.failed, [{ node_id: 0, message: "invalid config" }])
assert.equal(result.pending.length, 0)
assert.equal(attempts.get(1), 2)
assert.equal(attempts.get(2), 2)

// Even a transport ignoring abort must obey the overall deadline.
const timeout = await resolveCommandSync(report, () => new Promise(() => {}), { timeout: 25, interval: 1 })
assert.equal(timeout.pending.length, 12)
assert.equal(timeout.failed.length, 0)

const controller = new AbortController()
const pending = resolveCommandSync(report, () => new Promise(() => {}), { signal: controller.signal })
controller.abort()
await assert.rejects(pending, { name: "AbortError" })
console.log("Command sync concurrency, retry, failure, deadline and cancellation passed")
