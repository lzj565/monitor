"""HTTP + real Agent WebSocket reconciliation smoke test; disposable container only."""
import http.cookiejar, json, os, pathlib, re, socket, subprocess, tempfile, time, urllib.request
assert pathlib.Path('/.dockerenv').exists() and pathlib.Path('/tmp/monitor-mieru-disposable').exists()
hub_bin=os.environ.get('MIERU_HUB_BIN','/tmp/monitor-hub')
agent_bin=os.environ.get('MIERU_AGENT_BIN','/tmp/monitor-agent')
s=socket.socket();s.bind(('127.0.0.1',0));port=s.getsockname()[1];s.close()
base=f'http://127.0.0.1:{port}'
client=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
def request(path,body=None,method=None):
 headers={'content-type':'application/json','sec-fetch-site':'same-origin'}
 if method and method!='GET' and not path.endswith('/auth/login'):headers['origin']='https://monitor.example.com'
 req=urllib.request.Request(base+path,data=json.dumps(body).encode() if body is not None else None,headers=headers,method=method)
 with client.open(req,timeout=5) as r:
  data=r.read()
  return json.loads(data) if data and r.headers.get('content-type','').startswith('application/json') else data
with tempfile.TemporaryDirectory() as tmp:
 logpath=pathlib.Path(tmp)/'hub.log';agent=None
 with logpath.open('w') as log:
  hub=subprocess.Popen([hub_bin,f'127.0.0.1:{port}','--db',tmp+'/hub.db','--themes',tmp+'/themes'],stdout=log,stderr=log)
  try:
   for _ in range(150):
    try:request('/api/me');break
    except OSError:time.sleep(.1)
   else:raise AssertionError('Hub did not start')
   password=re.search(r'Emergency password: (\S+)',logpath.read_text())[1]
   request('/api/auth/login',{'password':password},'POST')
   node=request('/api/nodes',{'name':'mieru-management'},'POST')['id']
   token=request(f'/api/nodes/{node}/token')['token']
   proxy=request(f'/api/nodes/{node}/proxies',{'name':'mieru','protocol':'mieru','address_type':'ipv4','address':'127.0.0.1','port':24061,'enabled':True,'config':{'transport':'tcp'}},'POST')['id']
   admin=request('/api/user/me')['id']
   request(f'/api/users/{admin}/proxies/{proxy}',{'enabled':True},'PUT')
   (pathlib.Path('/opt/monitor')/'.skip-sing-box').touch()
   with open(tmp+'/agent.log','w') as agentlog:
    agent=subprocess.Popen([agent_bin,'--server',base,'--token',token],stdout=agentlog,stderr=agentlog)
    def await_state(running):
     for _ in range(250):
      hosts=request('/api/mieru/hosts')['items'];h=hosts[0]
      if h['status'].get('running')==running and h['revision']==h['applied_revision'] and not h['last_error']:return h
      time.sleep(.2)
     raise AssertionError(('reconciliation did not settle',h))
    await_state(True)
    portal=request('/api/user/me/proxies')['items'];assert len(portal)==1 and portal[0]['mieru_url'].startswith('mierus://')
    assert 'type: mieru' in portal[0]['mihomo_yaml'] and 'mieru_config' not in portal[0]
    links=request('/api/user/me/subscription');assert 'mieru' not in links
    assert b'type: mieru' in request(links['clash'])
    assert all(p['type']!='mieru' for p in request(links['sing_box'])['outbounds'])
    old=request('/api/mieru/credential')['password']
    assert request('/api/mieru/credential',{},'POST')['password']!=old
    time.sleep(6);await_state(True)
    # Revocation empties the generated user set and stops the service without manual intervention.
    request(f'/api/users/{admin}/proxies/{proxy}',None,'DELETE')
    time.sleep(6);await_state(False)
    print('HTTP authorization, native exports, credential rotation and real Agent reconciliation/revocation passed')
  finally:
   if agent is not None:agent.terminate();agent.wait(timeout=10)
   hub.terminate();hub.wait(timeout=10)
