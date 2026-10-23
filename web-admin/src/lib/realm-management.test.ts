import assert from "node:assert/strict"
import { normaliseRoute } from "./admin-route.ts"
import { proxyListItems, proxyTypeFilter, saveRealmRule, type RealmHost, type RealmRule, type RealmHostDraft } from "./realm-management.ts"
import type { Proxy } from "./resources.ts"
import type { Node } from "./api.ts"

const nodes = [{ id: 1, name: "入口", country: "HK" }, { id: 2, name: "出口", country: "JP" }] as Node[]
const proxy = { id: 1, node_id: 2, name: "Reality", include_node_name: true, enabled: true, address: "target.example", port: 443 } as Proxy
const host = { node_id: 1, public_address: "relay.example", enabled: true } as RealmHost
const rule = { id: 1, node_id: 1, name: "中转", listen_host: "0.0.0.0", listen_port: 24443, target_proxy_id: 1, target_node_id: 2, target_name: "Reality", target_address: "target.example", target_port: 443, target_enabled: true, enabled: true } as RealmRule
const filters = { query: "", node: "all", status: "all", type: "all" as const }
const list = (changes = {}) => proxyListItems([proxy], [rule], [host], nodes, { ...filters, ...changes })
assert.deepEqual(list().map((r) => r.key), ["proxy:1", "realm:1"], "overlapping numeric IDs stay distinct")
assert.deepEqual(list({ type: "realm" }).map((r) => r.kind), ["realm"])
assert.deepEqual(list({ node: "1" }).map((r) => r.kind), ["realm"])
assert.equal(list({ query: "relay.example" }).length, 1)
assert.equal(list({ query: "出口" }).length, 2, "target node search finds Realm forwarding")
assert.equal(list({ query: "target.example" }).length, 2)
assert.equal(list({ status: "disabled" }).length, 0)
assert.equal(proxyTypeFilter("?type=realm"), "realm")
assert.equal(proxyTypeFilter("?type=unknown"), "all")
assert.deepEqual(normaliseRoute("/admin/realm/", "?x=1"), { path: "/admin/proxies", search: "?x=1&type=realm" })
assert.deepEqual(normaliseRoute("/admin/proxies", "?type=vless"), { path: "/admin/proxies", search: "?type=vless" })
assert.equal(normaliseRoute("/admin/", "").path, "/admin/nodes")
assert.equal(normaliseRoute("/user/", "").path, "/user/center")

const draft = { node_id: 1, name: "中转", listen_host: "0.0.0.0", listen_port: 24443, target_proxy_id: 1, enabled: true }
const entry: RealmHostDraft = { node_id: 1, public_address: "relay.example", enabled: true, takeover: true }
const calls: { path: string; body: Record<string, unknown>; method: string }[] = []
async function request<T>(path: string, init: { body: string; method: string }): Promise<T> { calls.push({ path, body: JSON.parse(init.body), method: init.method }); return { id: 9 } as T }
await saveRealmRule(request, null, draft, entry)
assert.deepEqual(calls.map((c) => c.path), ["/realm/hosts", "/realm/rules"])
assert.equal(calls[1].body.protocol, undefined, "Realm must never be a sing-box protocol")
calls.length = 0
await saveRealmRule(request, 1, draft, entry, host)
assert.deepEqual(calls.map((c) => [c.path, c.method]), [["/realm/rules/1", "PUT"]], "reuse shared settings without rewriting them")
calls.length = 0
await saveRealmRule(request, null, draft, { ...entry, public_address: "new.example" }, host)
assert.equal(calls[0].path, "/realm/hosts")
let count = 0
await assert.rejects(saveRealmRule(async () => { count++; throw new Error("host failed") }, null, draft, entry), /host failed/)
assert.equal(count, 1, "failed entry configuration must not create a rule")
count = 0
await assert.rejects(saveRealmRule(async <T>(path: string) => { count++; if (path === "/realm/rules") throw new Error("port conflict"); return {} as T }, null, draft, entry), /port conflict/)
assert.equal(count, 2, "failed rule save must not delete shared settings")
await assert.rejects(saveRealmRule(request, null, draft, { ...entry, node_id: 2 }), /入口节点不一致/)
console.log("Unified Realm proxies: identities, filters, legacy routes and safe shared-entry save passed")
