# Run only inside the disposable container prepared for the ignored Agent mieru test.
# Requires /opt/monitor/mita, /usr/local/bin/mieru, /usr/local/bin/realm and the test systemctl wrapper.
import pathlib, socket, subprocess, time, json, threading
assert pathlib.Path('/.dockerenv').exists()
def echo_server():
 s=socket.socket();s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);s.bind(('0.0.0.0',24101));s.listen()
 while True:
  c,_=s.accept()
  def serve(c):
   try:
    data=c.recv(65536);c.sendall(data)
   finally:c.close()
  threading.Thread(target=serve,args=(c,),daemon=True).start()
threading.Thread(target=echo_server,daemon=True).start()
# mita intentionally blocks loopback/private destinations by default. Explicitly enable for this test only.
for _ in range(250):
 if pathlib.Path('/tmp/mieru-ready').exists():break
 time.sleep(.2)
else:raise AssertionError('Agent lifecycle test did not become ready')
conf=json.loads(pathlib.Path('/opt/monitor/config/mieru.json').read_text());conf['users'][0]['allowLoopbackIP']=True
pathlib.Path('/opt/monitor/config/mieru.json').write_text(json.dumps(conf))
env={'MITA_UDS_PATH':'/run/monitor-mieru/mita.sock','MITA_CONFIG_JSON_FILE':'/opt/monitor/config/mieru.json'}
import os
subprocess.run(['/usr/bin/systemctl','restart','monitor-mieru'],check=True,stdout=subprocess.DEVNULL)
client={'profiles':[{'profileName':'test','user':{'name':'user-1','password':'e2e-test-only-password'},'servers':[{'ipAddress':'127.0.0.1','portBindings':[{'port':24061,'protocol':'TCP'}]}]}],'activeProfile':'test','socks5Port':24100}
pathlib.Path('/tmp/mieru-client.json').write_text(json.dumps(client))
proc=subprocess.Popen(['/usr/local/bin/mieru','run'],env=dict(os.environ,MIERU_CONFIG_JSON_FILE='/tmp/mieru-client.json'),stdout=open('/tmp/mieru-client.log','w'),stderr=subprocess.STDOUT)
try:
 for _ in range(100):
  try:s=socket.create_connection(('127.0.0.1',24100),timeout=2);break
  except OSError:time.sleep(.1)
 else:raise AssertionError('client SOCKS port did not open')
 s.sendall(b'\x05\x01\x00');assert s.recv(2)==b'\x05\x00'
 s.sendall(b'\x05\x01\x00\x01\x7f\x00\x00\x01'+(24101).to_bytes(2,'big'))
 result=s.recv(10);assert result[:2]==b'\x05\x00',result
 payload=b'mieru-real-client-test-'*1024;s.sendall(payload);received=b''
 while len(received)<len(payload):
  chunk=s.recv(65536)
  if not chunk:break
  received+=chunk
 assert received==payload,(len(received),len(payload))
 s.close();time.sleep(.3)
 print('Native mieru TCP SOCKS echo round-trip passed:',len(payload),'bytes each direction')
 realm_config={'network':{'no_tcp':False,'use_udp':False},'endpoints':[{'listen':'127.0.0.1:24102','remote':'127.0.0.1:24061'}]}
 pathlib.Path('/tmp/mieru-realm.json').write_text(json.dumps(realm_config))
 relay=subprocess.Popen(['/usr/local/bin/realm','-c','/tmp/mieru-realm.json'],stdout=open('/tmp/mieru-realm.log','w'),stderr=subprocess.STDOUT)
 proc.terminate();proc.wait(timeout=10)
 client['profiles'][0]['servers'][0]['portBindings'][0]['port']=24102
 client['socks5Port']=24103
 pathlib.Path('/tmp/mieru-client.json').write_text(json.dumps(client))
 proc=subprocess.Popen(['/usr/local/bin/mieru','run'],env=dict(os.environ,MIERU_CONFIG_JSON_FILE='/tmp/mieru-client.json'),stdout=open('/tmp/mieru-client-relay.log','w'),stderr=subprocess.STDOUT)
 try:
  for _ in range(100):
   try:s=socket.create_connection(('127.0.0.1',24103),timeout=2);break
   except OSError:time.sleep(.1)
  else:raise AssertionError('Realm client SOCKS port did not open')
  s.sendall(b'\x05\x01\x00');assert s.recv(2)==b'\x05\x00'
  s.sendall(b'\x05\x01\x00\x01\x7f\x00\x00\x01'+(24101).to_bytes(2,'big'))
  assert s.recv(10)[:2]==b'\x05\x00'
  s.sendall(payload);received=b''
  while len(received)<len(payload):
   chunk=s.recv(65536)
   if not chunk:break
   received+=chunk
  assert received==payload
  s.close();print('Native mieru through Realm TCP echo round-trip passed:',len(payload),'bytes each direction')
 finally:relay.terminate();relay.wait(timeout=10)
 pathlib.Path('/tmp/mieru-traffic-done').write_text('done')
finally:proc.terminate();proc.wait(timeout=10)
