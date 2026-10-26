// Only use a disposable local Hub. Dependency: external playwright-core.
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { generateKeyPairSync } from "node:crypto"
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright-core")
const base=process.env.REALM_UI_BASE || "http://127.0.0.1:29931"
const container=process.env.REALM_UI_CONTAINER || "monitor-realm-proxy-ui"
assert.equal(new URL(base).hostname,"127.0.0.1")
assert.match(container,/^(monitor-realm-proxy-ui|realm-form-validation)(-[a-z0-9]+)?$/)
const logs=spawnSync("docker",["logs",container],{encoding:"utf8"})
assert.equal(logs.status,0,"temporary Hub container required")
const password=`${logs.stdout}${logs.stderr}`.match(/Emergency password: (\S+)/)?.[1]
assert.ok(password,"fresh test database required")
const browser=await chromium.launch({executablePath:process.env.CHROME_PATH || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",headless:true})
const context=await browser.newContext({viewport:{width:1440,height:1000}})
const page=await context.newPage();page.setDefaultTimeout(15000)
const errors=[];page.on("pageerror",e=>errors.push(e.message))
const writes=[];page.on("request",r=>{if(r.method()!=="GET"&&r.url().includes("/api/realm/"))writes.push({path:new URL(r.url()).pathname,method:r.method()})})
async function api(path,data,method=data?"POST":"GET"){
 const r=await context.request.fetch(base+"/api"+path,{method,data,headers:{origin:base,"sec-fetch-site":"same-origin"}})
 assert.ok(r.ok(),`${path}: ${r.status()} ${await r.text()}`);return r.status()===204?null:r.json()
}
async function until(check,message){const end=Date.now()+15000;while(Date.now()<end){if(await check())return;await new Promise(r=>setTimeout(r,100))}throw new Error(message)}
async function shot(path){await page.waitForFunction(()=>!document.querySelector('[data-state="closed"][data-slot="select-content"]'));await page.screenshot({path,animations:"disabled"})}
async function filter(label){await page.getByRole("combobox",{name:"按代理类型筛选"}).click();await page.getByRole("option",{name:label,exact:true}).click()}
async function newRealm(entry,target,proxy,name,port){
 await page.getByRole("button",{name:"新增代理",exact:true}).click()
 const d=page.getByRole("dialog");await d.getByRole("button",{name:"Realm 转发",exact:false}).click()
 await d.getByLabel("入口节点",{exact:true}).selectOption(String(entry));await d.getByLabel("代理名称",{exact:true}).fill(name)
 await d.getByLabel("监听端口",{exact:true}).fill(String(port));await d.getByLabel("目标节点",{exact:true}).selectOption(String(target));await d.getByLabel("目标代理",{exact:true}).selectOption(String(proxy));return d
}
try{
 await page.goto(base+"/admin/proxies");await page.getByLabel("管理员密码").fill(password);await page.getByRole("button",{name:"登录管理后台"}).click();await page.waitForURL("**/admin/nodes")
 const prefix=`realm-ui-${Date.now()}`
 const entry=(await api("/nodes",{name:prefix+"-entry"})).id
 const target=(await api("/nodes",{name:prefix+"-target"})).id
 const retryEntry=(await api("/nodes",{name:prefix+"-retry"})).id
 const v6Entry=(await api("/nodes",{name:prefix+"-v6"})).id
 await api(`/nodes/${entry}`,{ipv4_pin:"192.0.2.10",ipv6_pin:"2001:db8::10"},"PUT")
 await api(`/nodes/${v6Entry}`,{ipv6_pin:"2001:db8::20"},"PUT")
 const pair=generateKeyPairSync("x25519")
 const reality={enabled:true,server_name:"www.example.com",server_port:443,private_key:pair.privateKey.export({format:"jwk"}).d,public_key:pair.publicKey.export({format:"jwk"}).x,short_id:"abcd"}
 const targetName=prefix+"-Reality"
 const proxy=(await api(`/nodes/${target}/proxies`,{name:targetName,protocol:"vless",address_type:"domain",address:"target.example.com",port:443,enabled:true,config:{reality}})).id
 const localProxy=(await api(`/nodes/${entry}/proxies`,{name:prefix+"-local",protocol:"vless",address_type:"domain",address:"entry.example.com",port:443,enabled:true,config:{reality}})).id
 await page.goto(base+"/admin/realm/");await page.waitForURL("**/admin/proxies?type=realm")
 assert.equal(await page.locator("nav").getByRole("button",{name:"Realm 转发",exact:true}).count(),0)
 await filter("全部类型")
 await page.getByRole("button",{name:"新增代理",exact:true}).click()
 let d=page.getByRole("dialog");await d.getByRole("heading",{name:"选择代理类型"}).waitFor()
 const width=await d.evaluate(el=>el.offsetWidth)
 await d.getByRole("button",{name:/VLESS Reality/}).click()
 await d.getByRole("heading",{name:"新增 VLESS Reality 代理"}).waitFor()
 assert.equal(await d.evaluate(el=>el.offsetWidth),width)
 assert.equal(await page.getByRole("dialog").count(),1)
 await d.getByRole("button",{name:"返回选择类型"}).click();await d.getByRole("button",{name:/Realm 转发/}).click()
 await d.getByLabel("入口节点",{exact:true}).selectOption(String(v6Entry))
 assert.ok(await d.getByRole("button",{name:"IPv4",exact:true}).isDisabled())
 assert.equal(await d.getByRole("button",{name:"IPv6",exact:true}).getAttribute("aria-pressed"),"true")
 assert.equal(await d.getByLabel("对外连接地址（域名 / IP，不含端口）").inputValue(),"2001:db8::20")
 await d.getByLabel("入口节点",{exact:true}).selectOption(String(retryEntry))
 assert.ok(await d.getByRole("button",{name:"IPv6",exact:true}).isDisabled())
 assert.equal(await d.getByRole("button",{name:"自定义",exact:true}).getAttribute("aria-pressed"),"true")
 await d.getByLabel("入口节点",{exact:true}).selectOption(String(entry))
 assert.equal(await d.evaluate(el=>el.offsetWidth),width)
 assert.equal(await d.getByRole("button",{name:"IPv4",exact:true}).getAttribute("aria-pressed"),"true")
 assert.equal(await d.getByLabel("对外连接地址（域名 / IP，不含端口）").inputValue(),"192.0.2.10")
 assert.equal(await d.getByLabel("监听地址",{exact:true}).count(),0)
 assert.equal(await d.getByRole("checkbox",{name:/接管|入口节点的 Realm 服务/}).count(),0)
 assert.equal(await d.getByLabel("目标节点",{exact:true}).locator(`option[value="${entry}"]`).count(),0)
 const firstName=prefix+"-one"
 await d.getByLabel("代理名称",{exact:true}).fill(firstName);await d.getByLabel("监听端口",{exact:true}).fill("24443");await d.getByLabel("目标节点",{exact:true}).selectOption(String(target));await d.getByLabel("目标代理",{exact:true}).selectOption(String(proxy))
 await shot("/tmp/realm-form-v2-desktop.png")
 await d.getByRole("button",{name:"创建代理",exact:true}).click();await page.getByRole("dialog").waitFor({state:"hidden"})
 let first;await until(async()=>{first=(await api("/realm/rules")).items.find(r=>r.custom_name===firstName);return !!first},"rule missing")
 assert.equal(first.address,"192.0.2.10");assert.equal(first.address_type,"ipv4");assert.equal(first.listen_host,"::")
 let host=(await api("/realm/hosts")).items.find(h=>h.node_id===entry);assert.equal(host.enabled,true);assert.equal("public_address" in host,false)
 assert.equal(writes.filter(w=>w.path==="/api/realm/hosts").length,0)
 console.log("PASS: one equal-width wizard, family defaults, hidden listener/confirmation and automatic service initialization")
 const secondName=prefix+"-two";d=await newRealm(entry,target,proxy,secondName,24444)
 await d.getByRole("button",{name:"IPv6",exact:true}).click();await d.getByRole("button",{name:"创建代理",exact:true}).click();await page.getByRole("dialog").waitFor({state:"hidden"})
 let second=(await api("/realm/rules")).items.find(r=>r.custom_name===secondName);assert.equal(second.address,"2001:db8::10")
 await page.getByRole("button",{name:`编辑 Realm 代理 ${first.name}`,exact:true}).click();d=page.getByRole("dialog")
 assert.equal(await d.getByRole("button",{name:"返回选择类型"}).count(),0)
 await d.getByRole("button",{name:"自定义",exact:true}).click();await d.getByLabel("对外连接地址（域名 / IP，不含端口）").fill("custom.example.com")
 await d.getByLabel("监听端口",{exact:true}).fill("24444");await d.getByRole("button",{name:"保存修改",exact:true}).click();await d.getByRole("alert").filter({hasText:"端口"}).waitFor()
 await d.getByLabel("监听端口",{exact:true}).fill("24445");await d.getByRole("button",{name:"保存修改",exact:true}).click();await page.getByRole("dialog").waitFor({state:"hidden"})
 const updated=(await api("/realm/rules")).items
 assert.equal(updated.find(r=>r.id===first.id).address,"custom.example.com");assert.equal(updated.find(r=>r.id===first.id).traffic_key,first.traffic_key)
 assert.equal(updated.find(r=>r.id===second.id).address,"2001:db8::10")
 console.log("PASS: per-rule IPv4/IPv6/custom addresses, conflict validation and unchanged sibling/traffic identity")
 await page.getByRole("button",{name:`流量 / 入口服务 ${first.name}`,exact:true}).click();d=page.getByRole("dialog")
 assert.equal(await d.getByLabel("对外连接地址（不含端口）",{exact:true}).count(),0)
 assert.equal(await d.getByRole("checkbox").count(),0)
 await api("/realm/hosts",{node_id:entry,enabled:false},"PUT")
 await until(async()=>!(await api("/realm/hosts")).items.find(h=>h.node_id===entry).enabled,"service stop missing")
 await d.getByRole("button",{name:"关闭",exact:true}).click()
 const thirdName=prefix+"-stopped";d=await newRealm(entry,target,proxy,thirdName,24447)
 await d.getByText(/总开关已停用/).waitFor();await d.getByRole("button",{name:"创建代理",exact:true}).click();await page.getByRole("dialog").waitFor({state:"hidden"})
 assert.equal((await api("/realm/hosts")).items.find(h=>h.node_id===entry).enabled,false)
 console.log("PASS: details are read-only and a stopped service remains stopped on new rule creation")
 let failNext=true;await page.route("**/api/realm/rules",async route=>{if(failNext&&route.request().method()==="POST"){failNext=false;await route.fulfill({status:409,contentType:"application/json",body:JSON.stringify({error:{message:"test save failed"}})});return}await route.continue()})
 const retryName=prefix+"-retry-rule";d=await newRealm(retryEntry,target,proxy,retryName,24446)
 await d.getByLabel("对外连接地址（域名 / IP，不含端口）").fill("retry.example.com");await d.getByRole("button",{name:"创建代理",exact:true}).click();await d.getByRole("alert").filter({hasText:"test save failed"}).waitFor()
 assert.equal(await d.getByLabel("代理名称",{exact:true}).inputValue(),retryName);assert.ok(!(await api("/realm/hosts")).items.some(h=>h.node_id===retryEntry))
 await d.getByRole("button",{name:"创建代理",exact:true}).click();await page.getByRole("dialog").waitFor({state:"hidden"});assert.equal((await api("/realm/rules")).items.filter(r=>r.custom_name===retryName).length,1)
 await page.getByRole("button",{name:`删除 Realm 代理 ${first.name}`,exact:true}).click();await page.getByRole("dialog").getByRole("button",{name:"删除 Realm 代理",exact:true}).click();await page.getByRole("dialog").waitFor({state:"hidden"})
 assert.ok((await api("/proxies")).items.some(p=>p.id===proxy));assert.ok((await api("/realm/rules")).items.some(r=>r.id===second.id))
 await page.getByText("已删除 Realm 规则的历史流量",{exact:true}).click();await page.getByText(`${prefix}-entry / ${first.name}`,{exact:true}).waitFor()
 await filter("VLESS Reality");await page.getByRole("row").filter({hasText:targetName}).getByRole("button",{name:"编辑代理",exact:true}).click();d=page.getByRole("dialog");await d.locator("input").first().fill(targetName+"-edited");await d.getByRole("button",{name:"保存修改",exact:true}).click();await page.getByRole("dialog").waitFor({state:"hidden"})
 assert.equal((await api("/proxies")).items.find(p=>p.id===proxy).name,targetName+"-edited");assert.ok((await api("/proxies")).items.some(p=>p.id===localProxy))
 console.log("PASS: atomic failure/retry, Realm delete isolation/history and ordinary proxy editing")
 const mobileContext=await browser.newContext({viewport:{width:390,height:844},storageState:await context.storageState()});const mobile=await mobileContext.newPage()
 await mobile.goto(base+"/admin/proxies");await mobile.getByRole("button",{name:"新增代理",exact:true}).click();const md=mobile.getByRole("dialog");const mw=await md.evaluate(el=>el.offsetWidth)
 await md.getByRole("button",{name:/Realm 转发/}).click();await md.getByLabel("入口节点",{exact:true}).waitFor()
 assert.equal(await mobile.getByRole("dialog").count(),1);assert.equal(await md.evaluate(el=>el.offsetWidth),mw)
 await mobile.screenshot({path:"/tmp/realm-form-v2-mobile.png",animations:"disabled"});const bounds=await md.boundingBox();assert.ok(bounds.y>=0&&bounds.y+bounds.height<=845)
 const footer=await md.getByRole("button",{name:"创建代理",exact:true}).boundingBox();assert.ok(footer.y>=0&&footer.y+footer.height<=845)
 await md.getByLabel("目标代理",{exact:true}).scrollIntoViewIfNeeded();await mobile.screenshot({path:"/tmp/realm-form-v2-mobile-scrolled.png",animations:"disabled"})
 await mobileContext.close();assert.deepEqual(errors,[])
 console.log("PASS: mobile equal width, scrollable details and visible save button; no browser runtime errors")
}catch(e){await page.screenshot({path:"/tmp/realm-form-v2-failure.png"}).catch(()=>{});console.error(e.message);process.exitCode=1}finally{await browser.close()}
