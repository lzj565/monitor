// Local preview only; external playwright-core and Chrome are required.
import assert from 'node:assert/strict'
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright-core')
const base = process.env.PROXY_UI_BASE || 'http://127.0.0.1:29933'
assert.equal(new URL(base).hostname, '127.0.0.1')
const browser = await chromium.launch({ executablePath: process.env.CHROME_PATH || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true })
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
page.setDefaultTimeout(10000)
const ruleUpdates = []
const userUpdates = []
const errors = []; page.on('pageerror', e => errors.push(e.message))
const nodes = [1,2].map(id => ({ id, name: id === 1 ? '目标节点' : 'HK-ali', addresses: [{ address: `192.0.2.${id}` }], country: 'HK', group: '', online: true, public: true, metrics: null, last_seen_ago: 0, month_used: 0, traffic_limit: 0, traffic_reset_day: 1, price: 0, expires_at: null, iface: '' }))
const proxies = [1,2].map(id => ({ id, node_id: 1, name: id === 1 ? 'VLESS测试' : 'mieru测试', protocol: id === 1 ? 'vless' : 'mieru', address: 'target.example.com', address_type: 'domain', port: 24060+id, enabled: true, include_node_name: false, flow: '', config: { reality: { server_name: 'example.com' } } }))
const rules = [{ id: 1, node_id: 2, target_proxy_id: 2, name: 'HK-HK-HK家宽-realm', custom_name: 'HK家宽', address_type: 'domain', address: 'relay.example.com', listen_port: 24443, enabled: true, target_node_id: 1, target_name: 'mieru测试', target_address: 'target.example.com', target_port: 24062, target_enabled: true, traffic_key: 'test-rule' }]
const host = { node_id: 2, name: 'HK-ali', enabled: true, online: true, capable: true, traffic_capable: true, pending: false, desired_revision: 1, applied_revision: 1, last_error: '', status: { installed: true, managed: true, running: true, version: 'Realm 2.9.6 [brutal][batched-udp][proxy][balance]', target_reachability: 'reachable' } }
const traffic = { hosts: [{ node_id: 2, status: { state: 'monitoring', fresh: true, gaps: 0, discarded_records: 0, last_sample: 1793000000, up_rate: 0, down_rate: 0 } }], rules: [{ key: 'test-rule', node_id: 2, rule_id: 1, name: 'HK家宽-realm', active: true, up: 2589982, down: 5064622, today: 1541406, month: 7654604, last_sample: 1793000000, up_rate: 0, down_rate: 0 }] }
const users = [{ uuid: '11111111-2222-4333-8444-555555555555', auto_authorize_new_proxies: false, id: 7, username: 'test-user', enabled: true, proxy_count: 2, expires_at: null, traffic_limit: 0, device_limit: 0, traffic_reset_day: 0 }]
let password = 'fixture-shared-password'
let order = ['proxy:1','proxy:2','realm:1']; let failOrder = false; const saves = []; const writes = []
await page.routeWebSocket('**/api/ws', ws => ws.send(JSON.stringify({ nodes, admin: true })))
await page.route('**/api/**', async route => {
 const request = route.request(), path = new URL(request.url()).pathname
 let body = { items: [] }
 if (request.method() !== 'GET') writes.push(path)
 if (path === '/api/me') body = { authed: true, principal: 'admin', github: false, site_name: 'Monitor', public_page: true, site: '' }
 else if (path === '/api/nodes') body = { nodes, admin: true }
 else if (path === '/api/mieru/credential') { if (request.method() === 'POST') password = 'fixture-reset-password'; body = { password } }
 else if (path === '/api/proxies') body = { items: proxies }
 else if (path === '/api/users') body = { items: users }
 else if (path === '/api/users/7') {
  if (request.method() === 'PUT') { const data = request.postDataJSON(); userUpdates.push(data); Object.assign(users[0], data) }
  body = { ...users[0], sync: { queued: [], needs_sync: [], pending: [] } }
 }
 else if (path === '/api/users/7/uuid') { users[0].uuid = '99999999-8888-4777-8666-555555555555'; body = { uuid: users[0].uuid, sync: { queued: [], needs_sync: [], pending: [] } } }
 else if (path === '/api/users/7/proxies') body = { items: proxies.map(proxy => ({ proxy, access: { enabled: true, auth: { flow: "xtls-rprx-vision" } } })) }
 else if (path === '/api/realm/hosts') body = { items: [host] }
 else if (path === '/api/realm/rules') body = { items: rules }
 else if (path === '/api/realm/traffic') body = traffic
 else if (path === '/api/realm/rules/1' && request.method() === 'PUT') { ruleUpdates.push(request.postDataJSON()); body = { ok: true } }
 else if (path === '/api/proxy-list/order') {
  if (failOrder) { await route.fulfill({ status: 503, body: '排序读取失败' }); return }
  if (request.method() === 'PUT') { order = request.postDataJSON().ids; saves.push([...order]); body = { ok: true } }
  else body = { ids: order }
 }
 await route.fulfill({ contentType: 'application/json', body: JSON.stringify(body) })
})
try {
 await page.goto(base + '/admin/proxies')
 const toolbar = page.locator('[aria-label="代理筛选与操作"]')
 await page.getByRole('button', { name: '新增代理', exact: true }).waitFor()
 assert.equal(await page.getByRole('combobox', { name: '节点 Realm 服务', exact: true }).count(), 0)
 for (const width of [1440,1280,1024,375]) {
  await page.setViewportSize({ width, height: 1000 }); await page.waitForTimeout(100)
  const bounds = await toolbar.boundingBox()
  assert.ok(bounds.x >= 0 && bounds.x+bounds.width <= width)
  assert.ok(await toolbar.evaluate(e => e.scrollWidth <= e.clientWidth))
  if (width >= 1280) {
   const filter = await page.getByRole('combobox', { name: '按代理类型筛选' }).boundingBox()
   const add = await page.getByRole('button', { name: '新增代理', exact: true }).boundingBox()
   assert.ok(Math.abs(filter.y-add.y) < 2, `toolbar wraps at ${width}px`)
  }
 }
 await page.setViewportSize({ width: 1440, height: 1000 })
 await page.screenshot({ path: '/tmp/proxy-toolbar-desktop.png', animations: 'disabled' })
 await page.getByRole('button', { name: '新增代理', exact: true }).click()
 await page.getByRole('dialog').getByRole('button', { name: /mieru/ }).waitFor()
 await page.keyboard.press('Escape')

 await page.getByRole('button', { name: 'mieru 连接密码', exact: true }).click()
 const passwordDialog = page.getByRole('dialog')
 const passwordInput = passwordDialog.getByLabel('连接密码', { exact: true })
 await passwordInput.waitFor(); assert.equal(await passwordInput.getAttribute('type'), 'password')
 await passwordDialog.getByRole('button', { name: '显示密码', exact: true }).click()
 assert.equal(await passwordInput.inputValue(), password)
 assert.equal(await passwordDialog.getByLabel('连接用户名', { exact: true }).count(), 0)
 page.once('dialog', dialog => dialog.accept())
 await passwordDialog.getByRole('button', { name: '重置密码', exact: true }).click()
 await page.waitForFunction(() => document.querySelector('input[type="text"][readonly]')?.value === 'fixture-reset-password')
 await page.keyboard.press('Escape'); await passwordDialog.waitFor({ state: 'hidden' })

 for (const [type, label] of [['domain','自定义'],['ipv4','IPV4'],['ipv6','IPV6']]) {
  rules[0].address_type = type; rules[0].address = type === 'ipv6' ? '2001:db8::1' : '192.0.2.1'
  await page.reload(); await page.locator('tr[data-id="realm:1"]').getByText(label, { exact: true }).waitFor()
 }
 rules[0].address_type = 'domain'; rules[0].address = 'relay.example.com'; await page.reload()
 const realmHandle = page.getByRole('button', { name: '拖动 HK-HK-HK家宽-realm 排序', exact: true })
 await realmHandle.waitFor(); await page.waitForFunction(() => !document.querySelector('button[aria-label="拖动 HK-HK-HK家宽-realm 排序"]').disabled)
 // Real pointer drag moves Realm across both other protocol types.
 const source = await realmHandle.boundingBox(); const target = await page.getByRole('button', { name: '拖动 VLESS测试 排序', exact: true }).boundingBox()
 await page.mouse.move(source.x+source.width/2, source.y+source.height/2); await page.mouse.down(); await page.mouse.move(target.x+target.width/2, target.y+2, { steps: 12 }); await page.mouse.up()
 await page.waitForFunction(() => document.querySelector('tbody tr')?.dataset.id === 'realm:1')
 await page.waitForTimeout(300); assert.equal(saves.at(-1)?.[0], 'realm:1')
 await page.reload(); await realmHandle.waitFor(); assert.equal(await page.locator('tbody tr').first().getAttribute('data-id'), 'realm:1')
 await realmHandle.focus(); await page.keyboard.press('ArrowDown'); await page.waitForTimeout(300); assert.equal(saves.at(-1)?.[1], 'realm:1')
 await page.getByRole('button', { name: '编辑 Realm 代理 HK-HK-HK家宽-realm', exact: true }).click()
 const edit = page.getByRole('dialog'); assert.equal(await edit.getByLabel('代理名称', { exact: true }).inputValue(), 'HK家宽')
 await edit.getByRole('button', { name: '取消', exact: true }).click()
 await page.getByRole('switch', { name: '停用 Realm 代理 HK-HK-HK家宽-realm', exact: true }).click()
 await page.waitForTimeout(200); assert.equal(ruleUpdates.at(-1)?.name, 'HK家宽')
 await page.getByRole('combobox', { name: '按代理类型筛选' }).click(); await page.getByRole('option', { name: 'Realm 转发', exact: true }).click(); assert.ok(await realmHandle.isDisabled())
 await page.getByRole('button', { name: '流量 / 入口服务 HK-HK-HK家宽-realm', exact: true }).click()
 const dialog = page.getByRole('dialog'); await dialog.getByText('累计上传', { exact: true }).first().waitFor()
 assert.equal(await dialog.getByRole('switch').count(), 0); assert.equal(await dialog.getByRole('checkbox').count(), 0)
 assert.equal(await dialog.getByRole('button').count(), 2) // footer and accessible close icon
 assert.equal(await dialog.getByText(/brutal/).isVisible(), false)
 await page.screenshot({ path: '/tmp/realm-detail-desktop.png', animations: 'disabled' })
 await page.setViewportSize({ width: 375, height: 812 }); await page.waitForTimeout(250)
 assert.ok(await dialog.evaluate(e => { const r=e.getBoundingClientRect(); return r.left>=0 && r.right<=innerWidth && e.scrollWidth<=e.clientWidth && r.top>=0 && r.bottom<=innerHeight }))
 await page.screenshot({ path: '/tmp/realm-detail-mobile.png', animations: 'disabled' })
 await dialog.getByRole('button', { name: '关闭', exact: true }).click()
 await page.setViewportSize({ width: 1440, height: 1000 })
 host.online=false; host.pending=true; host.last_error='同步失败测试'; traffic.rules=[]; traffic.hosts[0].status.fresh=false
 await page.reload(); await page.getByRole('button', { name: '流量 / 入口服务 HK-HK-HK家宽-realm', exact: true }).click()
 await dialog.getByRole('alert').getByText('同步失败测试').waitFor()
 await dialog.getByText('节点离线，显示最近记录', { exact: true }).waitFor()
 assert.ok(await dialog.getByText('—', { exact: true }).count() >= 4)
 await dialog.getByRole('button', { name: '关闭', exact: true }).click()
 host.online=true; host.pending=false; host.last_error=''
 failOrder=true; await page.reload(); await page.getByText(/读取排序失败/).waitFor(); assert.ok(await realmHandle.isDisabled()); failOrder=false
 await page.goto(base + '/admin/subscriptions')
 const toggle = page.getByRole('button', { name: '展开test-user的节点', exact: true }); await toggle.waitFor()
 await page.getByText('test-user', { exact: true }).click(); assert.equal(await page.getByRole('button', { name: '收起test-user的节点' }).getAttribute('aria-expanded'), 'true')
 const row = page.getByRole('button', { name: '收起test-user的节点' }).locator('xpath=ancestor::tr'); await row.locator('td').nth(1).click({ position: { x: 220, y: 10 } }); assert.equal(await page.getByRole('button', { name: '展开test-user的节点' }).getAttribute('aria-expanded'), 'false')
 const collapsed = page.getByRole('button', { name: '展开test-user的节点' }); await collapsed.focus(); await page.keyboard.press('Space'); assert.equal(await page.getByRole('button', { name: '收起test-user的节点' }).getAttribute('aria-expanded'), 'true')
 await page.getByRole('switch', { name: '停用test-user的订阅授权', exact: true }).click(); assert.equal(await page.getByRole('button', { name: '收起test-user的节点' }).getAttribute('aria-expanded'), 'true')
 await page.getByRole('button', { name: '编辑 test-user 的授权节点', exact: true }).click(); await page.getByRole('dialog').waitFor(); assert.equal(await page.getByRole('button', { name: '收起test-user的节点', includeHidden: true }).getAttribute('aria-expanded'), 'true')
 await page.goto(base + '/admin/users'); await page.getByText('test-user', { exact: true }).waitFor(); assert.equal(await page.getByTitle('mieru 连接凭据', { exact: true }).count(), 0)
 const openUser = async () => { await page.getByRole('button', { name: '编辑用户 test-user', exact: true }).click(); const d=page.getByRole('dialog'); await d.getByLabel('UUID', { exact: true }).waitFor(); return d }
 let editUser = await openUser()
 const before = users[0].uuid
 await editUser.getByLabel('UUID', { exact: true }).fill('aaaaaaaa-bbbb-1ccc-8ddd-eeeeeeeeeeee')
 await editUser.getByRole('button', { name: '取消', exact: true }).click(); assert.equal(users[0].uuid, before)
 editUser = await openUser(); assert.equal(await editUser.getByLabel('UUID', { exact: true }).inputValue(), before)
 await editUser.getByLabel('UUID', { exact: true }).fill('bad-uuid'); const count = userUpdates.length
 await editUser.getByRole('button', { name: '保存修改', exact: true }).click(); assert.equal(userUpdates.length, count)
 await editUser.getByLabel('UUID', { exact: true }).fill(' AAAAAAAA-BBBB-1CCC-8DDD-EEEEEEEEEEEE ')
 page.once('dialog', d => d.dismiss()); await editUser.getByRole('button', { name: '保存修改', exact: true }).click(); assert.equal(userUpdates.length, count)
 page.once('dialog', d => d.accept()); await editUser.getByRole('button', { name: '保存修改', exact: true }).click(); await editUser.waitFor({ state: 'hidden' })
 assert.equal(userUpdates.at(-1).uuid, 'aaaaaaaa-bbbb-1ccc-8ddd-eeeeeeeeeeee')
 editUser = await openUser(); await editUser.getByRole('button', { name: '重置 UUID', exact: true }).click()
 const confirm = page.getByRole('dialog').filter({ has: page.getByRole('heading', { name: '确认重置 UUID？', exact: true }) })
 await confirm.getByRole('button', { name: '重置 UUID', exact: true }).click(); await confirm.waitFor({ state: 'hidden' })
 assert.equal(await editUser.getByLabel('UUID', { exact: true }).inputValue(), users[0].uuid)
 await editUser.getByRole('button', { name: '保存修改', exact: true }).click(); await editUser.waitFor({ state: 'hidden' })
 assert.equal(userUpdates.at(-1).uuid, undefined, 'saving after reset must not restore old UUID')
 assert.equal(users[0].uuid, '99999999-8888-4777-8666-555555555555')
 assert.ok(!writes.some(path => path.startsWith('/api/realm/hosts')))
 assert.deepEqual(errors, [])
 console.log('PASS: cross-protocol drag, persistence, keyboard, filtering and read failure; read-only responsive Realm details; broad subscription expansion; address badges and manual UUID cancel, validation, save, reset')
} finally { await browser.close() }
