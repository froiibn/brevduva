# Copyright 2026 SEIZIA (Jaeyoung Ko)
# SPDX-License-Identifier: Apache-2.0
"""Windows 실제 일반 Codex TUI로 queue 전달을 검증한다. receipt/reply는 MCP 호스트 fixture가 호출한다."""
import importlib.util,sys,os,json,tempfile,threading,time,subprocess,argparse
sys.dont_write_bytecode = True
parser=argparse.ArgumentParser()
parser.add_argument('--brv',default='target/debug/brv.exe')
parser.add_argument('--codex')
parser.add_argument('--deps',default='target/cli-probe-deps')
args=parser.parse_args()
from pathlib import Path
sys.path.insert(0,str(Path(args.deps).resolve()))
spec=importlib.util.spec_from_file_location('probe', 'crates/brv/tests/tools/probe_codex_cli.py'); m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
from winpty import PtyProcess
root=Path(tempfile.mkdtemp(prefix='brv-native-queue-'));work=root/'work';work.mkdir()
server=m.ThreadingHTTPServer(('127.0.0.1',0),m.Model);threading.Thread(target=server.serve_forever,daemon=True).start()
(root/'config.toml').write_text(f'''model="probe"
model_provider="probe"
approval_policy="never"
sandbox_mode="read-only"
[model_providers.probe]
name="Isolated model"
base_url="http://127.0.0.1:{server.server_port}/v1"
wire_api="responses"
requires_openai_auth=false
[projects.{json.dumps(str(work))}]
trust_level="trusted"
''')
env=dict(os.environ,CODEX_HOME=str(root),TERM='xterm-256color')
for k in ['OPENAI_API_KEY','CODEX_THREAD_ID','BREVDUVA_TOKEN','BREVDUVA_CONFIG','BREVDUVA_BINDING']:env.pop(k,None)
exe=args.codex or str(Path.home()/'AppData/Roaming/npm/node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe')
tui=PtyProcess.spawn([exe,'--no-alt-screen','PRIVATE_NATIVE_CONTEXT_853'],cwd=str(work),env=env,dimensions=(35,120));terminal=[]
def read():
 try:
  while tui.isalive():
   chunk=tui.read(8192);terminal.append(chunk)
   if '\x1b[6n' in chunk:tui.write('\x1b[1;1R')
   if '\x1b[c' in chunk:tui.write('\x1b[?1;2c')
 except (OSError,EOFError):pass
threading.Thread(target=read,daemon=True).start()
try:
 deadline=time.monotonic()+30
 while not m.requests and time.monotonic()<deadline:time.sleep(.2)
 assert m.requests, 'no initial model request'
 time.sleep(3)
 records=list(root.glob('sessions/**/*.jsonl'));assert len(records)==1,records
 first=json.loads(records[0].read_text(encoding='utf8').splitlines()[0]);tid=first['payload']['id']
 from websockets.sync.server import serve
 import queue
 mid='01ARZ3NDEKTSV4RRFFQ69G5FAV';frames=[];joined=[];replied=threading.Event()
 def relay(ws):
  join=json.loads(ws.recv());joined.append(join);ws.send(json.dumps({'op':'OK','re':join['seq'],'body':{}}))
  ws.send(json.dumps({'op':'DELIVER','seq':700,'body':{'v':1,'id':mid,'client_key':mid,'from':'peer','to':'agent:a','kind':'request','expects':'reply','hops':2,'content_type':'text/plain','payload':'UNTRUSTED_PAYLOAD_NATIVE','meta':{}}}))
  try:
   for raw in ws:
    frame=json.loads(raw);frames.append(frame)
    if frame['op']=='PUB':
     assert frame['body']['correlation_id']==mid and frame['body']['hops']==3
     ws.send(json.dumps({'op':'OK','re':frame['seq'],'body':{'id':'01ARZ3NDEKTSV4RRFFQ69G5FAW'}}));replied.set()
    elif frame['op']=='PING':ws.send(json.dumps({'op':'PONG','re':frame['seq']}))
  except Exception:pass
 relay_server=serve(relay,'127.0.0.1',0);threading.Thread(target=relay_server.serve_forever,daemon=True).start()
 configdir=root/'brv';configdir.mkdir();config=configdir/'config.toml';config.write_text(f'server="http://127.0.0.1:{relay_server.socket.getsockname()[1]}"\n[[binding]]\norg="test"\nagent="a"\nchannel="c"\n')
 menv=dict(env,BREVDUVA_TOKEN='isolated-fake-token',BREVDUVA_CONFIG=str(config))
 brv=str(Path(args.brv).resolve());log=open(root/'mcp-stderr.log','w')
 mcp=subprocess.Popen([brv,'mcp','--config',str(config),'--binding','test/a@c'],env=menv,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=log,text=True)
 responses=queue.Queue()
 def mcp_read():
  for line in mcp.stdout:responses.put(json.loads(line))
 threading.Thread(target=mcp_read,daemon=True).start()
 seq=0
 def call(method,params):
  global seq
  seq+=1;mcp.stdin.write(json.dumps({'jsonrpc':'2.0','id':seq,'method':method,'params':params})+'\n');mcp.stdin.flush()
  response=responses.get(timeout=20);assert response.get('id')==seq,response
  return response['result']
 def tool(name,args):
  r=call('tools/call',{'name':name,'arguments':args});assert not r.get('isError'),r
  return json.loads(r['content'][0]['text'])
 call('initialize',{'protocolVersion':'2025-06-18','clientInfo':{'name':'isolated-probe','version':'1'},'capabilities':{}})
 mcp.stdin.write(json.dumps({'jsonrpc':'2.0','method':'notifications/initialized'})+'\n');mcp.stdin.flush()
 activated=tool('receiver_connect',{'session_kind':'codex-cli','thread_id':tid,'codex_home':str(root),'codex_executable':exe})
 print('activated',activated['adapter'],activated['status'])
 deadline=time.monotonic()+20
 while not any('brevduva_message' in json.dumps(r) for r in m.requests) and time.monotonic()<deadline:time.sleep(.2)
 time.sleep(2)
 assert any('brevduva_message' in json.dumps(r) for r in m.requests)
 notice=None
 for req in m.requests:
  for item in req.get('input',[]):
   for c in item.get('content',[]):
    try:
     parsed=json.loads(c.get('text',''))
     if parsed.get('event')=='brevduva_message':notice=parsed
    except (ValueError,AttributeError):pass
 assert notice, 'native queue event did not reach the real TUI model'
 assert 'PRIVATE_NATIVE_CONTEXT_853' in json.dumps(m.requests[-1]), 'original context lost'
 assert 'UNTRUSTED_PAYLOAD_NATIVE' not in json.dumps(m.requests), 'peer payload promoted to queue input'
 assert tui.isalive(), 'original TUI exited'
 received=tool('receipt',{'message_id':notice['message_id'],'receipt_token':notice['receipt_token']})
 assert received['envelope']['payload']=='UNTRUSTED_PAYLOAD_NATIVE'
 tool('reply',{'to':'peer','correlation_id':mid,'payload':'reply from fixture'})
 assert replied.wait(5) and len(joined)==1 and any(f['op']=='ACK' for f in frames)
 print('receipt + durable ACK + same-connection reply: PASS (mock MCP host receipt)')
 mcp.stdin.close();mcp.wait(timeout=5);relay_server.shutdown();log.close()
 print(json.dumps({'root':str(root),'injected':any('brevduva_message' in json.dumps(r) for r in m.requests),'requests':len(m.requests),'tui_alive':tui.isalive(),'same_context':bool(len(m.requests)>1 and 'PRIVATE_NATIVE_CONTEXT_853' in json.dumps(m.requests[-1])),'rendered_count':''.join(terminal).count('PROBE_ACK')}))
finally:
 if 'mcp' in globals() and mcp.poll() is None:
  if not mcp.stdin.closed:mcp.stdin.close()
  try:mcp.wait(timeout=5)
  except subprocess.TimeoutExpired:mcp.kill();mcp.wait()
 if 'relay_server' in globals():relay_server.shutdown()
 if 'log' in globals():log.close()
 tui.close(force=True);server.shutdown();(root/'terminal.log').write_text(''.join(terminal),encoding='utf8');(root/'requests.json').write_text(json.dumps(m.requests),encoding='utf8')
