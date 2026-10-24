// UI regression with mocked APIs; requires external playwright-core and a local frontend preview.
import assert from 'node:assert/strict'
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE || 'playwright-core')
const base=process.env.COMPONENT_UI_BASE || 'http://127.0.0.1:29933';assert.equal(new URL(base).hostname,'127.0.0.1')
const browser=await chromium.launch({executablePath:process.env.CHROME_PATH || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true})
const context=await browser.newContext({viewport:{width:1440,height:1000},colorScheme:'light'});const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));page.setDefaultTimeout(10000)
const nodes=[{id:1,name:'安装选择测试节点',addresses:[{address:'192.0.2.1'}],country:'HK',group:'测试',online:false,public:true,metrics:null,last_seen_ago:null,month_used:0,traffic_limit:0,traffic_reset_day:1,price:0,expires_at:null,iface:''}]
let install={singbox:true,realm:true,mieru:false};let getFail=false,saveFail=false;const writes=[];const confirmations=[]
const profile=()=>({install,online:false,components:['singbox','realm','mieru'].map(key=>({component:key,desired_installed:install[key],installed:key!=='mieru',managed:key!=='mieru',running:false,version:key!=='mieru'?'fixture-v1':'',checked_at:123,last_error:''}))})
page.on('dialog',async d=>{confirmations.push(d.message());await d.accept()})
await page.routeWebSocket('**/api/ws',ws=>ws.send(JSON.stringify({nodes,admin:true})))
await page.route('**/api/**',async route=>{
 const r=route.request(),path=new URL(r.url()).pathname;let body={items:[]}
 if(path==='/api/me')body={authed:true,principal:'admin',github:false,site_name:'Monitor',public_page:true,site:'https://monitor.example.com'}
 else if(path==='/api/nodes')body={nodes,admin:true}
 else if(path==='/api/settings')body={site:'https://monitor.example.com',notify_traffic:80,register_key:'fixture-register',register_left:1800}
 else if(path==='/api/nodes/1/token')body={token:'fixture-token'}
 else if(path==='/api/nodes/1/components'){
  if((r.method()==='GET'&&getFail)||(r.method()==='PUT'&&saveFail)){await route.fulfill({status:503,contentType:'text/plain',body:'测试读取或保存失败'});return}
  if(r.method()==='PUT'){install=r.postDataJSON();writes.push(install)}body=profile()
 }
 await route.fulfill({contentType:'application/json',body:JSON.stringify(body)})
})
const command=d=>d.getByText(/curl -fsSL.*install\.sh/)
async function open(){await page.getByRole('button',{name:'安装 Agent',exact:true}).click();const d=page.getByRole('dialog');await d.getByRole('switch',{name:'安装 Realm'}).waitFor();return d}
async function close(d){await d.getByRole('button',{name:'关闭',exact:true}).click();await d.waitFor({state:'hidden'})}
try{
 await page.goto(base+'/admin/nodes');let d=await open();await command(d).waitFor()
 assert.equal(await d.getByRole('switch',{name:'安装 Realm'}).getAttribute('aria-checked'),'true');assert.equal(await d.getByRole('switch',{name:'安装 mieru'}).getAttribute('aria-checked'),'false')
 assert.equal(await d.getByRole('switch',{name:/sing-box/}).count(),0)
 assert.ok(!(await command(d).innerText()).includes('--no-realm'))
 await d.getByRole('switch',{name:'安装 mieru'}).click();assert.equal(await command(d).count(),0);assert.equal(await d.getByRole('button',{name:'复制',exact:true}).isEnabled(),false)
 await d.getByRole('button',{name:'保存安装选项'}).click();await command(d).waitFor();assert.deepEqual(writes[0],{singbox:true,realm:true,mieru:true})
 await close(d);d=await open();await command(d).waitFor();assert.equal(await d.getByRole('switch',{name:'安装 mieru'}).getAttribute('aria-checked'),'true')
 await d.getByRole('switch',{name:'安装 Realm'}).click();await d.getByRole('button',{name:'保存安装选项'}).click();await command(d).waitFor();assert.ok(confirmations.at(-1).includes('卸载 Realm'))
 await page.setViewportSize({width:375,height:667});await page.screenshot({path:'/tmp/components-install-mobile.png',animations:'disabled'})
 assert.ok(await d.evaluate(el=>el.scrollWidth<=el.clientWidth));await close(d)
 await page.reload();d=await open();await command(d).waitFor();assert.equal(await d.getByRole('switch',{name:'安装 Realm'}).getAttribute('aria-checked'),'false');await close(d)
 getFail=true;d=await open();await d.getByRole('alert').waitFor();assert.equal(await command(d).count(),0);assert.equal(await d.getByRole('button',{name:'复制',exact:true}).isEnabled(),false);await close(d);getFail=false
 d=await open();await command(d).waitFor();await d.getByRole('switch',{name:'安装 Realm'}).click();saveFail=true;await d.getByRole('button',{name:'保存安装选项'}).click();await d.getByRole('alert').waitFor();assert.equal(await command(d).count(),0);assert.equal(await d.getByRole('button',{name:'复制',exact:true}).isEnabled(),false)
 assert.deepEqual(errors,[]);console.log('PASS: saved choices restored after reopen/reload, sing-box locked, dirty/read/save errors hide commands, uninstall confirmed, 375px layout')
}finally{await browser.close()}
