/// <reference types="node" />
import assert from "node:assert/strict"
import { createResourceCache } from "./resource-cache.ts"

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: Error) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

const requests: ReturnType<typeof deferred<number[]>>[] = []
const cache = createResourceCache(() => {
  const request = deferred<number[]>()
  requests.push(request)
  return request.promise
})
let notifications = 0
const unsubscribe = cache.subscribe(() => { notifications += 1 })
assert.equal(cache.getSnapshot().items, null)
const initial = cache.refresh()
assert.equal(cache.refresh(), initial, "preloading and page entry share the pending request")
await Promise.resolve()
assert.equal(requests.length, 1)
requests[0].resolve([])
await initial
assert.deepEqual(cache.getSnapshot().items, [], "an empty list is a valid cache")

const refresh = cache.refresh()
assert.deepEqual(cache.getSnapshot().items, [], "background refresh retains the current list")
await Promise.resolve()
requests[1].resolve([1, 2])
await refresh

const old = cache.refresh()
await Promise.resolve()
const afterWrite = cache.refresh(true)
await Promise.resolve()
requests[3].resolve([3])
await afterWrite
requests[2].resolve([1, 2])
await old
assert.deepEqual(cache.getSnapshot().items, [3], "a response predating a write must not replace newer data")

const failure = cache.refresh()
await Promise.resolve()
requests[4].reject(new Error("network failed"))
await failure
assert.deepEqual(cache.getSnapshot().items, [3])
assert.equal(cache.getSnapshot().error, "network failed")
assert.equal(cache.getSnapshot().refreshing, false)
const retry = cache.refresh()
await Promise.resolve()
requests[5].resolve([4])
await retry
assert.equal(cache.getSnapshot().error, "")
assert.deepEqual(cache.getSnapshot().items, [4])

const cancelled = cache.refresh()
await Promise.resolve()
const beforeCancel = cache.getSnapshot()
cache.cancel()
requests[6].resolve([99])
await cancelled
assert.equal(cache.getSnapshot(), beforeCancel, "unmounted provider ignores pending responses")
const nextSession = createResourceCache(async () => [5])
assert.equal(nextSession.getSnapshot().items, null, "a new session starts with no cached data")
await nextSession.refresh()
assert.deepEqual(nextSession.getSnapshot().items, [5])
assert.ok(notifications > 0)
unsubscribe()

const initialFailure = createResourceCache<number>(async () => { throw new Error("unavailable") })
await initialFailure.refresh()
assert.equal(initialFailure.getSnapshot().items, null)
assert.equal(initialFailure.getSnapshot().error, "unavailable")
assert.equal(initialFailure.getSnapshot().refreshing, false)
console.log("Resource cache deduplication, retention, invalidation, failures and session isolation passed")
