"""Real Hub + Agent integration with controlled component/service fixtures, disposable Docker only."""
import http.cookiejar, json, os, pathlib, re, socket, subprocess, tempfile, time, urllib.error, urllib.request
assert pathlib.Path('/.dockerenv').exists() and pathlib.Path('/tmp/components-disposable').exists()
root=pathlib.Path('/opt/monitor');assert not root.exists(), 'refusing existing installation'
(root/'config').mkdir(parents=True);pathlib.Path('/run/systemd/system').mkdir(parents=True,exist_ok=True);pathlib.Path('/etc/systemd/system').mkdir(exist_ok=True)
(root/'sing-box').write_text('#!/bin/sh\ncase "$1" in version) echo "sing-box fixture-v1" ;; esac\nexit 0\n');(root/'sing-box').chmod(0o755)
(root/'config/sb.json').write_text('{}')
pathlib.Path('/etc/systemd/system/sing-box.service').write_text('ExecStart=/opt/monitor/sing-box run -c /opt/monitor/config/sb.json\n')
pathlib.Path('/usr/bin/systemctl').write_text('''#!/bin/sh
case "$1" in
 is-active) test -f /opt/monitor/sing-box.running && { test "$2" = sing-box || { test "$2" = --quiet && test "$3" = sing-box; }; } ;;
 restart) touch /opt/monitor/sing-box.running ;;
 *) exit 0 ;;
esac
''');pathlib.Path('/usr/bin/systemctl').chmod(0o755)
s=socket.socket();s.bind(('127.0.0.1',0));port=s.getsockname()[1];s.close();base=f'http://127.0.0.1:{port}'
client=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
def request(path,body=None,method=None,headers=None,expected=200,opener=client):
 h={'content-type':'application/json','sec-fetch-site':'same-origin'}
 if method and method!='GET' and not path.endswith('/auth/login'):h['origin']='https://monitor.example.com'
 h.update(headers or {})
 req=urllib.request.Request(base+path,data=json.dumps(body).encode() if body is not None else None,headers=h,method=method)
 try:r=opener.open(req,timeout=10)
 except urllib.error.HTTPError as e:r=e
 with r:
  data=r.read();assert r.status==expected,(path,r.status,data[:200])
  return json.loads(data) if data and r.headers.get('content-type','').startswith('application/json') else data.decode()
helper=pathlib.Path('/tmp/install.sh').read_text().split('# BEGIN component profile helpers')[1].split('# END component profile helpers')[0]
pathlib.Path('/tmp/component-helper.sh').write_text(helper)
with tempfile.TemporaryDirectory() as tmp:
 logpath=pathlib.Path(tmp)/'hub.log';agent=None;hub=None
 def start_hub():
  log=open(logpath,'a');p=subprocess.Popen(['/tmp/monitor-hub',f'127.0.0.1:{port}','--db',tmp+'/hub.db','--themes',tmp+'/themes'],stdout=log,stderr=log);log.close()
  for _ in range(150):
   try:request('/api/me');return p
   except OSError:time.sleep(.1)
  raise AssertionError('Hub did not start')
 try:
  hub=start_hub();password=re.search(r'Emergency password: (\S+)',logpath.read_text())[1];request('/api/auth/login',{'password':password},'POST')
  node=request('/api/nodes',{'name':'component-fixture'},'POST')['id'];token=request(f'/api/nodes/{node}/token')['token']
  initial=request(f'/api/nodes/{node}/components');assert initial['install']=={'singbox':True,'realm':False,'mieru':False};assert all(c['checked_at'] is None for c in initial['components'])
  chosen={'singbox':True,'realm':True,'mieru':True};saved=request(f'/api/nodes/{node}/components',chosen,'PUT');assert saved['install']==chosen and all(not c['installed'] for c in saved['components'])
  request('/api/agent/components',expected=401)
  assert request('/api/agent/components',headers={'authorization':'Bearer '+token})=='singbox=1\nrealm=1\nmieru=1\n'
  # Exercise the actual installer parser against this live authenticated HTTP endpoint.
  env=dict(os.environ,SERVER=base,TOKEN=token,TMP_DIR=tmp,REALM_OPTION='',MIERU_OPTION='')
  shell='set -eu; info() { :; }; fail() { exit 1; }; . /tmp/component-helper.sh; load_component_profile; printf "%s %s %s" "$SINGBOX_OPTION" "$REALM_OPTION" "$MIERU_OPTION"'
  result=subprocess.run(['/bin/sh','-c',shell],env=env,capture_output=True,text=True);assert result.returncode==0 and result.stdout=='--sing-box --realm --mieru',result.stderr
  request(f'/api/nodes/{node}/components',{'singbox':False,'realm':False,'mieru':False},'PUT',expected=400)
  with open(tmp+'/agent.log','w') as agentlog:agent=subprocess.Popen(['/tmp/monitor-agent','--server',base,'--token',token],stdout=agentlog,stderr=agentlog)
  for _ in range(150):
   profile=request(f'/api/nodes/{node}/components')
   if all(c['checked_at'] is not None for c in profile['components']):break
   time.sleep(.1)
  else:raise AssertionError('component inventory did not arrive')
  state={c['component']:c for c in profile['components']}
  assert state['singbox']['installed'] and state['singbox']['managed'];assert state['singbox']['version']=='sing-box fixture-v1'
  assert not state['realm']['installed'] and not state['mieru']['installed'];assert profile['install']==chosen
  public=request('/api/nodes',opener=urllib.request.build_opener());assert 'components' not in json.dumps(public)
  agent.terminate();agent.wait(timeout=10);agent=None;hub.terminate();hub.wait(timeout=10);hub=start_hub()
  assert request(f'/api/nodes/{node}/components')['install']==chosen
  print('PASS: defaults, saved choices independent of observed files, live installer parser, mandatory sing-box, real Agent inventory, public isolation and Hub restart persistence')
 finally:
  if agent is not None:agent.terminate();agent.wait(timeout=10)
  if hub is not None:hub.terminate();hub.wait(timeout=10)
