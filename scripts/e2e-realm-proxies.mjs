// Run only against an isolated local Hub with a disposable database. Browser
// dependency is external: PLAYWRIGHT_MODULE=/tmp/.../playwright-core/index.mjs.
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { generateKeyPairSync } from "node:crypto"
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright-core")
const base = process.env.REALM_UI_BASE || "http://127.0.0.1:29931"
assert.equal(new URL(base).hostname, "127.0.0.1", "local disposable Hub only")
const container = process.env.REALM_UI_CONTAINER || "monitor-realm-proxy-ui"
assert.match(container, /^monitor-realm-proxy-ui/)
const logs = spawnSync("docker", ["logs", container], { encoding: "utf8" })
assert.equal(logs.status, 0, "test Hub container must exist")
const password = `${logs.stdout}${logs.stderr}`.match(/Emergency password: (\S+)/)?.[1]
assert.ok(password, "fresh test database emergency password missing")
const browser = await chromium.launch({ executablePath: process.env.CHROME_PATH || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true })
const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } })
const page = await context.newPage()
page.setDefaultTimeout(15000)
const errors = []
page.on("pageerror", (e) => errors.push(e.message))
const writes = []
page.on("request", (r) => { if (r.method() !== "GET" && r.url().includes("/api/realm/")) writes.push({ path: new URL(r.url()).pathname, method: r.method() }) })
async function api(path, data, method = data ? "POST" : "GET") {
  const response = await context.request.fetch(base + "/api" + path, { method, data, headers: { origin: base, "sec-fetch-site": "same-origin" } })
  assert.ok(response.ok(), `${path}: ${response.status()} ${await response.text()}`)
  return response.status() === 204 ? null : response.json()
}
async function until(check, message) { const deadline = Date.now() + 15000; while (Date.now() < deadline) { if (await check()) return; await new Promise((r) => setTimeout(r, 100)) } throw new Error(message) }
async function screenshot(path) {
  await page.waitForFunction(() => !document.querySelector('[data-state="closed"][data-slot="dialog-content"], [data-state="closed"][data-slot="select-content"]'))
  await page.screenshot({ path, animations: "disabled" })
}
async function typeFilter(value) {
  await page.getByRole("combobox", { name: "按代理类型筛选" }).click()
  await page.getByRole("option", { name: value, exact: true }).click()
}
async function newRealm(entryId, targetId, proxyId, name, port) {
  await page.getByRole("button", { name: "新增代理", exact: true }).click()
  await page.getByRole("dialog").getByRole("combobox").first().click()
  await page.getByRole("option", { name: "Realm 转发", exact: true }).click()
  const d = page.getByRole("dialog")
  await d.getByLabel("入口节点", { exact: true }).selectOption(String(entryId))
  await d.getByLabel("代理名称", { exact: true }).fill(name)
  await d.getByLabel("监听端口", { exact: true }).fill(String(port))
  await d.getByLabel("目标节点", { exact: true }).selectOption(String(targetId))
  await d.getByLabel("目标代理", { exact: true }).selectOption(String(proxyId))
  return d
}
try {
  await page.goto(base + "/admin/proxies")
  await page.getByLabel("管理员密码").fill(password)
  await page.getByRole("button", { name: "登录管理后台" }).click()
  await page.waitForURL("**/admin/nodes")
  const prefix = `realm-ui-${Date.now()}`
  const entry = (await api("/nodes", { name: prefix + "-entry" })).id
  const target = (await api("/nodes", { name: prefix + "-target" })).id
  const failedEntry = (await api("/nodes", { name: prefix + "-retry" })).id
  const pair = generateKeyPairSync("x25519")
  const reality = { enabled: true, server_name: "www.example.com", server_port: 443, private_key: pair.privateKey.export({ format: "jwk" }).d, public_key: pair.publicKey.export({ format: "jwk" }).x, short_id: "abcd" }
  const targetName = prefix + "-Reality"
  const proxy = (await api(`/nodes/${target}/proxies`, { name: targetName, protocol: "vless", address_type: "domain", address: "target.example.com", port: 443, enabled: true, config: { reality } })).id
  const localProxy = (await api(`/nodes/${entry}/proxies`, { name: prefix + "-local", protocol: "vless", address_type: "domain", address: "entry.example.com", port: 443, enabled: true, config: { reality } })).id
  await page.goto(base + "/admin/realm/")
  await page.waitForURL("**/admin/proxies?type=realm")
  assert.equal(await page.locator("nav").getByRole("button", { name: "Realm 转发", exact: true }).count(), 0)
  await typeFilter("全部类型")
  const firstName = prefix + "-relay-one"
  let d = await newRealm(entry, target, proxy, firstName, 24443)
  assert.equal(await d.getByText("Reality 密钥", { exact: true }).count(), 0)
  assert.equal(await d.getByLabel("监听地址", { exact: true }).inputValue(), "0.0.0.0")
  assert.equal(await d.getByLabel("目标节点", { exact: true }).locator(`option[value="${entry}"]`).count(), 0)
  await d.getByLabel("对外连接地址（域名 / IP，不含端口）", { exact: true }).fill("relay.example.com")
  await d.getByRole("checkbox", { name: /确认由面板接管/ }).check()
  await screenshot("/tmp/realm-proxies-form-desktop.png")
  await page.setViewportSize({ width: 390, height: 844 })
  await screenshot("/tmp/realm-proxies-form-mobile.png")
  await page.setViewportSize({ width: 1440, height: 1000 })
  await d.getByRole("button", { name: "创建代理", exact: true }).click()
  await page.getByRole("dialog").waitFor({ state: "hidden" })
  let first
  await until(async () => { first = (await api("/realm/rules")).items.find((r) => r.name === firstName); return !!first }, "first Realm missing")
  await page.getByRole("row").filter({ hasText: firstName }).waitFor()
  assert.equal(writes.filter((w) => w.path === "/api/realm/hosts").length, 1)
  assert.ok((await api("/proxies")).items.some((p) => p.id === localProxy))
  console.log("PASS: legacy route, unified list, node selection and first-use Realm creation")

  const secondName = prefix + "-relay-two"
  d = await newRealm(entry, target, proxy, secondName, 24444)
  assert.equal(await d.getByRole("checkbox", { name: /确认由面板接管/ }).count(), 0)
  await d.getByRole("button", { name: "创建代理", exact: true }).click()
  await page.getByRole("dialog").waitFor({ state: "hidden" })
  assert.equal(writes.filter((w) => w.path === "/api/realm/hosts").length, 1, "existing entry settings must not be rewritten")
  await page.getByRole("button", { name: `编辑 Realm 代理 ${firstName}`, exact: true }).click()
  d = page.getByRole("dialog")
  await d.getByLabel("监听端口", { exact: true }).fill("24444")
  await d.getByRole("button", { name: "保存修改", exact: true }).click()
  await d.getByRole("alert").filter({ hasText: "监听端口" }).waitFor()
  await d.getByLabel("监听端口", { exact: true }).fill("24445")
  await d.getByRole("button", { name: "保存修改", exact: true }).click()
  await page.getByRole("dialog").waitFor({ state: "hidden" })
  assert.equal((await api("/realm/rules")).items.find((r) => r.id === first.id).traffic_key, first.traffic_key)
  await page.getByRole("switch", { name: `停用 Realm 代理 ${firstName}`, exact: true }).click()
  await until(async () => !(await api("/realm/rules")).items.find((r) => r.id === first.id).enabled, "rule not disabled")
  assert.equal((await api("/proxies")).items.find((p) => p.id === proxy).enabled, true)
  await page.getByRole("switch", { name: `启用 Realm 代理 ${firstName}`, exact: true }).click()
  console.log("PASS: shared entry reuse, port conflict, stable traffic identity and isolated rule toggle")

  await page.getByRole("button", { name: `流量 / 入口服务 ${firstName}`, exact: true }).click()
  d = page.getByRole("dialog")
  await d.getByLabel("对外连接地址（不含端口）", { exact: true }).fill("relay-new.example.com")
  assert.ok(await d.getByRole("button", { name: "保存共享设置" }).isDisabled())
  await d.getByRole("checkbox", { name: /确认修改节点共享设置/ }).check()
  await d.getByRole("button", { name: "保存共享设置" }).click()
  await until(async () => (await api("/realm/hosts")).items.find((h) => h.node_id === entry).public_address === "relay-new.example.com", "shared address not saved")
  await d.getByText(/累计 ↑ —/).first().waitFor()
  assert.ok(await d.getByRole("button", { name: "重新检测流量监控" }).isDisabled())
  await d.getByRole("button", { name: "刷新 / 重试" }).click()
  assert.equal(writes.filter((w) => w.path.endsWith("/operations")).length, 1)
  await d.getByRole("button", { name: "Close", exact: true }).click()
  console.log("PASS: shared settings confirmation, unavailable rates and service operations")

  let failNext = true
  await page.route("**/api/realm/rules", async (route) => {
    if (failNext && route.request().method() === "POST") { failNext = false; await route.fulfill({ status: 409, contentType: "application/json", body: JSON.stringify({ error: { code: "TEST_CONFLICT", message: "test rule save failed" } }) }); return }
    await route.continue()
  })
  const retriedName = prefix + "-relay-retry"
  d = await newRealm(failedEntry, target, proxy, retriedName, 24446)
  await d.getByLabel("对外连接地址（域名 / IP，不含端口）", { exact: true }).fill("retry.example.com")
  await d.getByRole("checkbox", { name: /确认由面板接管/ }).check()
  await d.getByRole("button", { name: "创建代理", exact: true }).click()
  await d.getByRole("alert").filter({ hasText: "test rule save failed" }).waitFor()
  assert.equal(await d.getByLabel("代理名称", { exact: true }).inputValue(), retriedName)
  assert.ok((await api("/realm/hosts")).items.some((h) => h.node_id === failedEntry))
  await d.getByRole("button", { name: "创建代理", exact: true }).click()
  await page.getByRole("dialog").waitFor({ state: "hidden" })
  assert.equal((await api("/realm/rules")).items.filter((r) => r.name === retriedName).length, 1)
  assert.equal(writes.filter((w) => w.path.includes("/realm/hosts/") && w.method === "DELETE").length, 0)
  console.log("PASS: partial save preserves shared settings and rule draft; retry creates one rule")

  await page.getByRole("button", { name: `删除 Realm 代理 ${firstName}`, exact: true }).click()
  await page.getByRole("dialog").getByRole("button", { name: "删除 Realm 代理", exact: true }).click()
  await page.getByRole("dialog").waitFor({ state: "hidden" })
  await until(async () => !(await api("/realm/rules")).items.some((r) => r.id === first.id), "rule not deleted")
  assert.ok((await api("/proxies")).items.some((p) => p.id === proxy))
  assert.ok((await api("/realm/rules")).items.some((r) => r.name === secondName))
  await page.getByText("已删除 Realm 规则的历史流量", { exact: true }).click()
  await page.getByText(`${prefix}-entry / ${firstName}`, { exact: true }).waitFor()
  await typeFilter("VLESS Reality")
  const ordinary = page.getByRole("row").filter({ hasText: targetName })
  await ordinary.getByRole("button", { name: "编辑代理", exact: true }).click()
  d = page.getByRole("dialog")
  await d.locator("input").first().fill(targetName + "-edited")
  await d.getByRole("button", { name: "保存修改", exact: true }).click()
  await page.getByRole("dialog").waitFor({ state: "hidden" })
  assert.equal((await api("/proxies")).items.find((p) => p.id === proxy).name, targetName + "-edited")
  await typeFilter("全部类型")
  await page.locator(".overflow-x-auto").evaluateAll((elements) => elements.forEach((e) => { e.scrollLeft = 0 }))
  await screenshot("/tmp/realm-proxies-list-desktop.png")
  await page.setViewportSize({ width: 390, height: 844 })
  await screenshot("/tmp/realm-proxies-list-mobile.png")
  const mobileContext = await browser.newContext({ viewport: { width: 390, height: 844 }, storageState: await context.storageState() })
  const mobile = await mobileContext.newPage()
  await mobile.goto(base + "/admin/proxies")
  await mobile.getByRole("button", { name: "新增代理", exact: true }).click()
  await mobile.getByRole("dialog").getByRole("combobox").first().click()
  await mobile.getByRole("option", { name: "Realm 转发", exact: true }).click()
  await mobile.getByRole("dialog").getByLabel("入口节点", { exact: true }).waitFor()
  await mobile.waitForFunction(() => document.querySelectorAll('[data-slot="dialog-content"]').length === 1)
  await mobile.screenshot({ path: "/tmp/realm-proxies-form-mobile-fresh.png", animations: "disabled" })
  const bounds = await mobile.getByRole("dialog").boundingBox()
  assert.ok(bounds.y >= 0 && bounds.y + bounds.height <= 845, `mobile dialog outside viewport: ${JSON.stringify(bounds)}`)
  const footer = await mobile.getByRole("dialog").getByRole("button", { name: "创建代理", exact: true }).boundingBox()
  assert.ok(footer.y >= 0 && footer.y + footer.height <= 845, "mobile save button outside viewport")
  await mobile.getByRole("dialog").getByLabel("目标代理", { exact: true }).scrollIntoViewIfNeeded()
  await mobile.screenshot({ path: "/tmp/realm-proxies-form-mobile-scrolled.png", animations: "disabled" })
  await mobileContext.close()
  assert.deepEqual(errors, [], "unexpected browser errors")
  console.log("PASS: Realm deletion preserves target/history; ordinary proxy editing and mobile layouts")
} catch (e) {
  await page.screenshot({ path: "/tmp/realm-proxies-ui-failure.png" }).catch(() => {})
  console.error(e.message)
  process.exitCode = 1
} finally { await browser.close() }
