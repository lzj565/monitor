import assert from "node:assert/strict"
import { realmArg } from "./realm.ts"
assert.deepEqual(realmArg(true), ["--realm"])
assert.deepEqual(realmArg(false), ["--no-realm"])
assert.deepEqual(realmArg(undefined), [])
console.log("Realm install, uninstall and legacy command options passed")

import { trafficBytes, trafficState } from "./realm-traffic.ts"
assert.equal(trafficBytes(null), "—")
assert.equal(trafficBytes(0), "0 B")
assert.equal(trafficBytes(1024), "1.00 KiB")
assert.match(trafficState({ state: "unmonitored", reason: "missing nft" }), /未监控.*missing nft/)
assert.match(trafficState({ state: "monitoring", fresh: false }), /过期/)
