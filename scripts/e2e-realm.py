import subprocess,tempfile,pathlib,time,socket,threading,urllib.request,urllib.error,http.cookiejar,json,re,os,tarfile
assert pathlib.Path('/.dockerenv').exists(), 'Run this test only in a disposable Docker container'
assert not pathlib.Path('/opt/monitor/config').exists(), 'Refusing to replace existing monitor files'
root=pathlib.Path('/opt/monitor');root.mkdir(parents=True,exist_ok=True);(root/'config').mkdir(exist_ok=True)
with tarfile.open('/realm.tar.gz') as archive:
 member=next(m for m in archive.getmembers() if m.name.removeprefix('./')=='realm')
 (root/'realm').write_bytes(archive.extractfile(member).read())
(root/'realm').chmod(0o755);(root/'.realm-managed').write_text('')
(root/'config/realm.json').write_text('{"log":{"level":"warn"},"endpoints":[]}')
pathlib.Path('/run/systemd/system').mkdir(parents=True,exist_ok=True)
pathlib.Path('/etc/systemd/system/monitor-realm.service').write_text('# Managed by monitor installer\n')
binpath=pathlib.Path('/tmp/realm-e2e-bin');binpath.mkdir(exist_ok=True)
service='''#!/bin/sh
cd /opt/monitor
case "$1" in
 is-active) test -f realm.pid && kill -0 "$(cat realm.pid)" 2>/dev/null ;;
 is-enabled) test -f enabled ;;
 enable) touch enabled ;;
 disable) rm -f enabled ;;
 start|restart)
  if test -f realm.pid; then kill "$(cat realm.pid)" 2>/dev/null || true; sleep .1; fi
  /opt/monitor/realm -c /opt/monitor/config/realm.json >> /tmp/realm-service.log 2>&1 &
  echo $! > realm.pid
  ;;
 stop) if test -f realm.pid; then kill "$(cat realm.pid)" 2>/dev/null || true; rm realm.pid; fi ;;
 *) exit 1 ;;
esac
'''
(binpath/'systemctl').write_text(service);(binpath/'systemctl').chmod(0o755)
def free_port():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
echo=socket.socket();echo.bind(('127.0.0.1',0));echo.listen();target_port=echo.getsockname()[1]
def echo_loop():
 while True:
  try:
   conn,_=echo.accept()
   with conn:
    conn.settimeout(2);data=conn.recv(1024)
    if data:conn.sendall(data)
  except OSError:pass
threading.Thread(target=echo_loop,daemon=True).start()
def client():return urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
def request(opener,path,body=None,method=None):
 headers={'content-type':'application/json','sec-fetch-site':'same-origin'}
 if method and method!='GET' and not path.endswith('/auth/login'):headers['origin']='https://monitor.example.com'
 req=urllib.request.Request(base+path,data=json.dumps(body).encode() if body is not None else None,headers=headers,method=method)
 try:
  with opener.open(req,timeout=10) as r:
   content=r.read();return r.status,json.loads(content) if content and r.headers.get('content-type','').startswith(('application/json','application/yaml')) and path.endswith('/clash')==False else content
 except urllib.error.HTTPError as e:raise AssertionError((path,e.code,e.read().decode()))
with tempfile.TemporaryDirectory() as d:
 hublog=open(d+'/hub.log','w+');agentlog=open(d+'/agent.log','w+');port=free_port();base='http://127.0.0.1:'+str(port);admin=client();agent=None
 hub=subprocess.Popen(['/hub/target/debug/monitor-hub','127.0.0.1:'+str(port),'--db',d+'/hub.db','--themes',d+'/themes'],stdout=hublog,stderr=hublog)
 try:
  for _ in range(200):
   try:
    if request(admin,'/api/me')[0]==200:break
   except OSError:pass
   time.sleep(.05)
  hublog.flush();password=re.search(r'Emergency password: (\S+)',pathlib.Path(d+'/hub.log').read_text())[1]
  assert request(admin,'/api/auth/login',{'password':password},'POST')[0]==200
  source=request(admin,'/api/nodes',{'name':'realm-entry'},'POST')[1]['id'];target=request(admin,'/api/nodes',{'name':'realm-target'},'POST')[1]['id']
  token=request(admin,f'/api/nodes/{source}/token')[1]['token']
  proxy=request(admin,f'/api/nodes/{target}/proxies',{'name':'test-Reality','protocol':'vless','address_type':'ipv4','address':'127.0.0.1','port':target_port,'enabled':True,'config':{'reality':{'enabled':True,'server_name':'www.example.com','server_port':443,'private_key':'A'*43,'public_key':'B'*43,'short_id':'abcd'}}},'POST')[1]['id']
  uid=request(admin,'/api/users',{'username':'realm-e2e','password':'temporary-test-password','enabled':True},'POST')[1]['id']
  request(admin,f'/api/users/{uid}/proxies/{proxy}',{'enabled':True},'PUT')
  request(admin,'/api/realm/hosts',{'node_id':source,'enabled':True},'PUT')
  listen_port=free_port();rule=request(admin,'/api/realm/rules',{'node_id':source,'target_proxy_id':proxy,'name':'test-route','address_type':'ipv4','address':'127.0.0.1','listen_port':listen_port,'enabled':True},'POST')[1]['id']
  # Optional controlled failure backend: count every invocation and refuse it.
  if os.environ.get('REALM_TEST_NO_NFT'):
   (binpath/'nft').write_text('#!/bin/sh\necho probe >> /tmp/nft-probes\necho unavailable >&2\nexit 1\n');(binpath/'nft').chmod(0o755)
  env=os.environ.copy();env['PATH']=str(binpath)+':'+env['PATH']
  agent=subprocess.Popen(['/agent/target/debug/monitor-agent','--server',base,'--token',token],env=env,stdout=agentlog,stderr=agentlog)
  for _ in range(200):
   host=request(admin,'/api/realm/hosts')[1]['items'][0]
   if host['applied_revision']>0 and not host['pending'] and host['status'].get('running'):break
   time.sleep(.2)
  else:raise AssertionError(('realm did not apply',host,pathlib.Path(d+'/agent.log').read_text()[-1500:]))
  with socket.create_connection(('127.0.0.1',listen_port),timeout=3) as conn:
   conn.sendall(b'realm-real-binary-e2e');assert conn.recv(1024)==b'realm-real-binary-e2e'
  if os.environ.get('REALM_TEST_TRAFFIC') or os.environ.get('REALM_TEST_NO_NFT'):
   def traffic_status():
    hosts=request(admin,'/api/realm/traffic')[1]['hosts']
    return next((h['status'] for h in hosts if h['node_id']==source),{})
   for _ in range(200):
    stats=traffic_status()
    if stats.get('state') in ('monitoring','unmonitored'):break
    time.sleep(.2)
   if os.environ.get('REALM_TEST_NO_NFT'):
    assert stats.get('state')=='unmonitored', stats
    before=pathlib.Path('/tmp/nft-probes').read_text().count('probe')
    request(admin,f'/api/realm/hosts/{source}/operations',{'action':'retry'},'POST')
    time.sleep(7)
    assert pathlib.Path('/tmp/nft-probes').read_text().count('probe')==before
    agent.terminate();agent.wait(timeout=10)
    agent=subprocess.Popen(['/agent/target/debug/monitor-agent','--server',base,'--token',token],env=env,stdout=agentlog,stderr=agentlog)
    time.sleep(7)
    assert pathlib.Path('/tmp/nft-probes').read_text().count('probe')==before
    request(admin,f'/api/realm/hosts/{source}/operations',{'action':'traffic_recheck'},'POST')
    for _ in range(200):
     if pathlib.Path('/tmp/nft-probes').read_text().count('probe')>before:break
     time.sleep(.2)
    else:raise AssertionError('manual recheck did not probe')
    with socket.create_connection(('127.0.0.1',listen_port),timeout=3) as conn:
     conn.sendall(b'forwarding-still-works');assert conn.recv(1024)==b'forwarding-still-works'
    print('PASS: unavailable nft latched across refresh and Agent restart; manual recheck retries; forwarding unaffected')
   else:
    assert stats.get('state')=='monitoring',stats
    # External client runs in a second isolated container; do not count loopback.
    pathlib.Path('/tmp/realm-traffic-port').write_text(str(listen_port))
    for _ in range(300):
     data=request(admin,'/api/realm/traffic')[1]
     if data['rules'] and data['rules'][0]['up']>0 and data['rules'][0]['down']>0:break
     time.sleep(.2)
    else:raise AssertionError(('no externally generated traffic',data))
    totals=(data['rules'][0]['up'],data['rules'][0]['down'])
    for _ in range(10):
     with socket.create_connection(('127.0.0.1',listen_port),timeout=3) as conn:
      conn.sendall(b'loopback-not-counted');assert conn.recv(1024)==b'loopback-not-counted'
    time.sleep(6)
    data=request(admin,'/api/realm/traffic')[1]
    assert (data['rules'][0]['up'],data['rules'][0]['down'])==totals,('loopback counted',totals,data)
    print('PASS: real nft input/output counters received by Hub; both directions positive; loopback excluded')
    original_key=data['rules'][0]['key'];listen_port=free_port()
    request(admin,f'/api/realm/rules/{rule}',{'node_id':source,'target_proxy_id':proxy,'name':'test-route-edited','address_type':'ipv4','address':'127.0.0.1','listen_port':listen_port,'enabled':True},'PUT')
    for _ in range(200):
     host=request(admin,'/api/realm/hosts')[1]['items'][0]
     if not host['pending'] and host['status'].get('running'):break
     time.sleep(.2)
    else:raise AssertionError(('port edit did not apply',host))
    pathlib.Path('/tmp/realm-traffic-port').write_text(str(listen_port))
    for _ in range(300):
     data=request(admin,'/api/realm/traffic')[1]
     if data['rules'][0]['up']>totals[0] and data['rules'][0]['down']>totals[1]:break
     time.sleep(.2)
    else:raise AssertionError(('no traffic on edited port',data))
    assert data['rules'][0]['key']==original_key
    print('PASS: editing listener port preserves rule identity and cumulative traffic')
  user=client();request(user,'/api/user/auth/login',{'username':'realm-e2e','password':'temporary-test-password'},'POST')
  items=request(user,'/api/user/me/proxies')[1]['items'];assert len(items)==2 and any(p['id']==-rule for p in items)
  links=request(user,'/api/user/me/subscription')[1];clash=request(user,links['clash'])[1];assert b'127.0.0.1' in clash and ('Realm' in clash.decode())
  sing=request(user,links['sing_box'])[1];assert len([p for p in sing['outbounds'] if p['type']=='vless'])==2
  request(admin,'/api/realm/hosts',{'node_id':source,'enabled':False},'PUT')
  assert len(request(user,'/api/user/me/proxies')[1]['items'])==1
  for _ in range(160):
   host=request(admin,'/api/realm/hosts')[1]['items'][0]
   if not host['pending'] and host['status'].get('running') is False:break
   time.sleep(.2)
  else:raise AssertionError(('realm did not stop',host))
  request(admin,f'/api/realm/rules/{rule}',method='DELETE')
  history=request(admin,'/api/realm/traffic')[1]['rules'];assert history and history[0]['active'] is False
  print('PASS: real Realm v2.9.6 + new Agent + Hub: auto apply, TCP echo forwarding, authorized portal + Clash/sing-box entries, immediate withdrawal and confirmed stop')
 finally:
  if agent:agent.terminate();agent.wait(timeout=10)
  hub.terminate();hub.wait(timeout=10)
  if (root/'realm.pid').exists():
   try:os.kill(int((root/'realm.pid').read_text()),15)
   except ProcessLookupError:pass
  hublog.close();agentlog.close()
