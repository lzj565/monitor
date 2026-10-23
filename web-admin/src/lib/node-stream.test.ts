import assert from "node:assert/strict"
import { setTimeout as sleep } from "node:timers/promises"
import { createNodeStream, type NodeFrame } from "./node-stream.ts"

function socket() {
  const fake = {
    onmessage: null as ((event: MessageEvent) => void) | null,
    onclose: null as (() => void) | null,
    onerror: null as (() => void) | null,
    close() { fake.onclose?.() },
  }
  return fake
}
function frame(admin: boolean): NodeFrame { return { nodes: [], admin } }

// An HTTP reply that started before a stream update cannot replace that update.
let finish!: (value: NodeFrame) => void
const updates: NodeFrame[] = []
const errors: Error[] = []
const firstSocket = socket()
let stoppedSignal!: AbortSignal
const stop = createNodeStream({
  fetchNodes: (signal) => { stoppedSignal = signal; return new Promise((resolve) => { finish = resolve }) },
  openSocket: () => firstSocket as unknown as WebSocket,
  onFrame: (value) => updates.push(value),
  onError: (error) => errors.push(error),
})
firstSocket.onmessage?.({ data: JSON.stringify(frame(true)) } as MessageEvent)
finish(frame(false))
await sleep(0)
assert.deepEqual(updates, [frame(true)])
const lateMessage = firstSocket.onmessage!
stop()
assert.equal(stoppedSignal.aborted, true)
lateMessage({ data: JSON.stringify(frame(false)) } as MessageEvent)
assert.equal(updates.length, 1, "cleanup rejects queued socket messages")

// A stalled poll does not start more requests; a synchronous socket error retries.
let active = 0
let maximum = 0
let calls = 0
let connects = 0
const stopPolling = createNodeStream({
  fetchNodes: async () => {
    calls += 1
    maximum = Math.max(maximum, ++active)
    await sleep(25)
    active -= 1
    return frame(false)
  },
  openSocket: () => { connects += 1; throw new Error("unsupported") },
  onFrame: () => {}, onError: () => {}, delay: 5,
})
await sleep(80)
stopPolling()
assert.equal(maximum, 1)
assert.ok(calls > 1)
assert.ok(connects > 1, "a constructor error also schedules reconnects")

// Invalid JSON is contained and reconnects rather than killing the event handler.
const brokenSocket = socket()
const stopBroken = createNodeStream({
  fetchNodes: async () => frame(false), openSocket: () => brokenSocket as unknown as WebSocket,
  onFrame: () => {}, onError: (error) => errors.push(error),
})
brokenSocket.onmessage?.({ data: "not json" } as MessageEvent)
assert.match(errors.at(-1)!.message, /格式错误/)
stopBroken()
console.log("Node stream ordering, cancellation, non-overlapping polling and malformed frames passed")

// A socket that never closes must still be retired by the silence watchdog.
let silentConnects = 0
const silentSocket = socket()
silentSocket.close = () => {} // Native close events may take up to a minute.
const stopSilent = createNodeStream({
  fetchNodes: async () => frame(false),
  openSocket: () => { silentConnects += 1; return silentSocket as unknown as WebSocket },
  onFrame: () => {}, onError: () => {}, silence: 10, delay: 5,
})
await sleep(35)
stopSilent()
assert.ok(silentConnects > 1, "silence reconnects without waiting for close")

// Backgrounding aborts old work; foregrounding fetches immediately. A queued
// response from the old page lifetime cannot replace the resumed page's data.
let hidden = false
let visibilityChanged!: () => void
let oldReply!: (value: NodeFrame) => void
let visibilitySignal!: AbortSignal
let visibilityCalls = 0
const visibleFrames: NodeFrame[] = []
const stopVisible = createNodeStream({
  fetchNodes: (signal) => {
    visibilityCalls += 1
    if (visibilityCalls === 1) {
      visibilitySignal = signal
      return new Promise((resolve) => { oldReply = resolve })
    }
    return Promise.resolve(frame(true))
  },
  openSocket: () => socket() as unknown as WebSocket,
  onFrame: (value) => visibleFrames.push(value), onError: () => {},
  visibility: { hidden: () => hidden, subscribe: (listener) => { visibilityChanged = listener; return () => {} } },
})
hidden = true
visibilityChanged()
assert.equal(visibilitySignal.aborted, true)
hidden = false
visibilityChanged()
await sleep(0)
oldReply(frame(false))
await sleep(0)
stopVisible()
assert.equal(visibilityCalls, 2)
assert.deepEqual(visibleFrames, [frame(true)])
console.log("Node stream silence watchdog and foreground recovery passed")
