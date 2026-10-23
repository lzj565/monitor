import assert from "node:assert/strict"
import { normaliseRoute } from "./admin-route.ts"
import { proxyListItems, proxyTypeFilter, saveRealmRule, defaultProxyAddressType, nodeProxyAddress, type RealmRule } from "./realm-management.ts"
import type { Proxy } from "./resources.ts"
import type { Node } from "./api.ts"

const nodes = [{ id: 1, name: "入口", country: "HK" }, { id: 2, name: "出口", country: "JP" }] as Node[]
const proxy = { id: 1, node_id: 2, name: "Reality", include_node_name: true, enabled: true, address: "target.example", port: 443 } as Proxy
const rule = { id: 1, node_id: 1, name: "中转", address_type: "domain", address: "relay.example", listen_port: 24443, target_proxy_id: 1, target_node_id: 2, target_name: "Reality", target_address: "target.example", target_port: 443, target_enabled: true, enabled: true } as RealmRule
const filters = { query: "", node: "all", status: "all", type: "all" as const }
const list = (changes = {}) => proxyListItems([proxy], [rule], nodes, { ...filters, ...changes })
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

const dualNode = { id: 3, addresses: [{address:"2001:db8::1"},{address:"192.0.2.1"}] } as Node
assert.equal(defaultProxyAddressType(dualNode),"ipv4")
assert.equal(nodeProxyAddress(dualNode,"ipv6"),"2001:db8::1")
assert.equal(defaultProxyAddressType({addresses:[{address:"2001:db8::1"}]} as Node),"ipv6")
assert.equal(defaultProxyAddressType({addresses:[]} as unknown as Node),"domain")
const draft = { node_id: 1, name: "中转", address_type: "domain" as const, address: "relay.example", listen_port: 24443, target_proxy_id: 1, enabled: true }
const calls: { path: string; body: Record<string, unknown>; method: string }[] = []
async function request<T>(path: string, init: { body: string; method: string }): Promise<T> { calls.push({ path, body: JSON.parse(init.body), method: init.method }); return { id: 9 } as T }
await saveRealmRule(request, null, draft)
assert.deepEqual(calls.map((c)=>c.path),["/realm/rules"])
assert.equal(calls[0].body.protocol,undefined)
assert.equal(calls[0].body.listen_host,undefined)
assert.equal(calls[0].body.takeover,undefined)
assert.equal(calls[0].body.address,"relay.example")
calls.length=0
await saveRealmRule(request,1,{...draft,address_type:"ipv6",address:"2001:db8::1"})
assert.deepEqual(calls.map((c)=>[c.path,c.method]),[["/realm/rules/1","PUT"]])
assert.equal(calls[0].body.address,"2001:db8::1")
let count=0
await assert.rejects(saveRealmRule(async()=>{count++;throw new Error("port conflict")},null,draft),/port conflict/)
assert.equal(count,1,"a failed save never writes or deletes shared node settings")
console.log("Realm per-rule addresses, filters, default families and single-request save passed")
