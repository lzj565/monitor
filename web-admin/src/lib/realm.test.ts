import assert from "node:assert/strict"
import { realmArg } from "./realm.ts"
assert.deepEqual(realmArg(true), ["--realm"])
assert.deepEqual(realmArg(false), ["--no-realm"])
assert.deepEqual(realmArg(undefined), [])
console.log("Realm install, uninstall and legacy command options passed")
