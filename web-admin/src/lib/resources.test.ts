/// <reference types="node" />
import assert from "node:assert/strict"

import { generateRealityKeyPair, generateShortId, realityKeyPairMatches } from "./reality.ts"
import { proxyDraft } from "./proxy-draft.ts"
import { countryFlag, displayProxyName } from "./proxy-name.ts"
import { formatSni, parseSni } from "./sni.ts"
import { existingAuthorizationSettings, groupProxiesByNode, proxyGroupSelection, subscriptionSearchMatches, toggleProxyGroup } from "./subscription.ts"
import type { Node } from "./api.ts"
import type { Proxy } from "./resources.ts"

const pair = generateRealityKeyPair()
assert.match(pair.privateKey, /^[A-Za-z0-9_-]{43}$/)
assert.match(pair.publicKey, /^[A-Za-z0-9_-]{43}$/)
assert.equal(realityKeyPairMatches(pair.privateKey, pair.publicKey), true)
const otherPair = generateRealityKeyPair()
assert.equal(realityKeyPairMatches(pair.privateKey, otherPair.publicKey), false)
assert.match(generateShortId(), /^[0-9a-f]{8}$/)
assert.deepEqual(parseSni("www.amd.com:443"), { server_name: "www.amd.com", server_port: 443 })
assert.deepEqual(parseSni("www.amd.com"), { server_name: "www.amd.com", server_port: 443 })
assert.deepEqual(parseSni("example.com:8443"), { server_name: "example.com", server_port: 8443 })
assert.deepEqual(parseSni("[2001:db8::1]:443"), { server_name: "2001:db8::1", server_port: 443 })
assert.equal(formatSni("2001:db8::1", 443), "[2001:db8::1]:443")
assert.throws(() => parseSni("example.com:65536"), /SNI 端口/)
assert.throws(() => parseSni("2001:db8::1"), /IPv6 SNI/)

const proxy: Proxy = {
  id: 4,
  node_id: 2,
  name: "香港 Reality",
  include_node_name: false,
  protocol: "vless",
  address_type: "domain",
  address: "hk.example.com",
  port: 24060,
  enabled: true,
  config: {
    reality: {
      enabled: true,
      server_name: "www.apple.com",
      server_port: 443,
      private_key: pair.privateKey,
      public_key: pair.publicKey,
      short_id: "abcdef12",
    },
  },
  created_at: 10,
  updated_at: 11,
}
const disabledDraft = proxyDraft(proxy, false)
assert.deepEqual(disabledDraft, {
  name: proxy.name,
  include_node_name: proxy.include_node_name,
  protocol: proxy.protocol,
  address_type: proxy.address_type,
  address: proxy.address,
  port: proxy.port,
  enabled: false,
  config: proxy.config,
})
assert.equal(proxy.enabled, true, "building a switch PUT draft must not mutate the listed proxy")

assert.equal(countryFlag("hk"), "🇭🇰")
assert.equal(countryFlag(""), "")
assert.equal(displayProxyName({ ...proxy, include_node_name: true }, { name: "HK服务器", country: "HK" }), "🇭🇰 [HK服务器] 香港 Reality")
assert.equal(displayProxyName({ ...proxy, include_node_name: true }, { name: "无地区", country: "" }), "[无地区] 香港 Reality")

const hk = { id: 2, name: "HK服务器", country: "HK" } as Node
const jp = { id: 1, name: "JP服务器", country: "JP" } as Node
const secondProxy: Proxy = { ...proxy, id: 5, node_id: 1, name: "Tokyo", address_type: "ipv6", address: "2400:xxxx::1", port: 443 }
const groups = groupProxiesByNode([proxy, secondProxy], [hk, jp])
assert.deepEqual(groups.map((group) => group.nodeId), [2, 1], "groups follow the displayed node order")
assert.deepEqual(proxyGroupSelection([proxy.id], [proxy.id, secondProxy.id]), { selected: 1, total: 2, checked: false, indeterminate: true })
assert.deepEqual(toggleProxyGroup([proxy.id], [proxy.id, secondProxy.id]), [proxy.id, secondProxy.id], "selecting a partial group selects all")
assert.deepEqual(toggleProxyGroup([proxy.id, secondProxy.id], [proxy.id, secondProxy.id]), [], "selecting a full group clears it")

const access = {
  proxy: { id: proxy.id, node_id: proxy.node_id, name: proxy.name, include_node_name: true, protocol: proxy.protocol, address_type: proxy.address_type, address: proxy.address, port: proxy.port, enabled: proxy.enabled },
  access: { enabled: false, auth: { flow: "xtls-rprx-vision" as const } },
}
const settings = existingAuthorizationSettings([access])
assert.deepEqual(settings[proxy.id], { flow: access.access.auth.flow, enabled: false }, "edit form preserves per-authorization flow and enabled state")
const proxyById = new Map([[proxy.id, proxy]])
const nodeById = new Map([[hk.id, hk]])
assert.equal(subscriptionSearchMatches("admin", [access], proxyById, nodeById, "hk服务器"), true, "subscription search includes server names")
assert.equal(subscriptionSearchMatches("admin", [access], proxyById, nodeById, "hk.example.com"), true, "subscription search includes proxy addresses")
assert.equal(subscriptionSearchMatches("admin", [access], proxyById, nodeById, "admin"), true, "subscription search includes usernames")
assert.equal(subscriptionSearchMatches("other", [access], proxyById, nodeById, "missing"), false)

console.log("Reality, proxy drafts, subscription grouping, selection, search, and authorization preservation passed")
