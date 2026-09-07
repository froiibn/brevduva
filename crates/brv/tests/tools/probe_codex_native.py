# Copyright 2026 SEIZIA (Jaeyoung Ko)
# SPDX-License-Identifier: Apache-2.0
"""Windows 일반 Codex TUI + 로컬 모의 모델/WS의 MCP 활성화·자동 수신·회신 시험.
격리 fixture의 MCP 도구 승인만 처리하며 사용자 설정·계정·세션을 변경하지 않는다.
실제 모델의 추론 품질 시험은 아니다.
"""
import argparse, json, os, re, sys, tempfile, threading, time
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
sys.dont_write_bytecode = True
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--brv', default='target/debug/brv.exe')
parser.add_argument('--codex', default=str(Path.home()/'AppData/Roaming/npm/node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe'))
parser.add_argument('--deps', default='target/cli-probe-deps')
args = parser.parse_args()
sys.path.insert(0,str(Path(args.deps).resolve()))
from winpty import PtyProcess
from websockets.sync.server import serve
root=Path(tempfile.mkdtemp(prefix='brv-codex-full-'));work=root/'work';work.mkdir()
mid='01ARZ3NDEKTSV4RRFFQ69G5FAV';trigger=threading.Event();replied=threading.Event();requests=[];frames=[];phase='start';terminal=[]
def relay(ws):
 try:
  join=json.loads(ws.recv(timeout=40));frames.append(join);ws.send(json.dumps({'op':'OK','re':join['seq'],'body':{}}))
  assert trigger.wait(40)
  ws.send(json.dumps({'op':'DELIVER','seq':700,'body':{'v':1,'id':mid,'client_key':mid,'from':'peer','to':'agent:a','kind':'request','expects':'reply','hops':2,'content_type':'text/plain','payload':'CODEX_NATIVE_PEER_492','meta':{}}}))
  for raw in ws:
   f=json.loads(raw);frames.append(f)
   if f['op']=='PUB':
    assert f['body']['correlation_id']==mid and f['body']['hops']==3 and f['body']['payload']=='CODEX_NATIVE_REPLY',f
    ws.send(json.dumps({'op':'OK','re':f['seq'],'body':{'id':'01ARZ3NDEKTSV4RRFFQ69G5FAW'}}));replied.set()
   elif f['op']=='PING':ws.send(json.dumps({'op':'PONG','re':f['seq']}))
 except Exception as e:frames.append({'error':repr(e)})
relayserver=serve(relay,'127.0.0.1',0);threading.Thread(target=relayserver.serve_forever,daemon=True).start()
brvdir=root/'brv';brvdir.mkdir();config=brvdir/'config.toml';config.write_text(f'server="http://127.0.0.1:{relayserver.socket.getsockname()[1]}"\n[[binding]]\norg="test"\nagent="a"\nchannel="c"\n')
brv=Path(args.brv).resolve()
def strings(v):
 if isinstance(v,str):yield v
 elif isinstance(v,list):
  for x in v:yield from strings(x)
 elif isinstance(v,dict):
  for x in v.values():yield from strings(x)
class Model(BaseHTTPRequestHandler):
 def log_message(self,*a):pass
 def do_POST(self):
  global phase
  r=json.loads(self.rfile.read(int(self.headers['Content-Length'])));requests.append({'path':self.path,'body':r})
  names=[x.get('name','') for x in r.get('tools',[])] + [ns['name']+'.'+x['name'] for ns in r.get('tools',[]) for x in ns.get('tools',[])];flat='\n'.join(strings(r.get('input',[])));name=None;arguments={};text='READY_NATIVE'
  if 'ACTIVATE_NATIVE_931' in flat:
   connect=next((x for x in names if x.endswith('receiver_connect')),None)
   if phase=='start' and connect:
    records=list(root.glob('sessions/**/*.jsonl'));tid=json.loads(records[0].read_text().splitlines()[0])['payload']['id']
    name=connect;arguments={'session_kind':'codex-cli','thread_id':tid,'codex_home':str(root),'codex_executable':exe};phase='connect'
   elif phase=='connect' and 'codex-queue' in flat:
    phase='armed';text='ARMED_NATIVE'
   elif phase=='armed':
    found=re.search(r'"receipt_token"\s*:\s*"([A-Z0-9]{26})"',flat)
    if found:
     name=next(x for x in names if x.endswith('.receipt'));arguments={'message_id':mid,'receipt_token':found.group(1)};phase='receipt'
   elif phase=='receipt' and 'CODEX_NATIVE_PEER_492' in flat:
    name=next(x for x in names if x.endswith('.reply'));arguments={'to':'peer','correlation_id':mid,'payload':'CODEX_NATIVE_REPLY'};phase='reply'
   elif phase=='reply':phase='done';text='DELIVERED_NATIVE'
  ident='item_'+str(len(requests));rid='resp_'+str(len(requests))
  item={'id':ident,'type':'function_call','call_id':'call_'+str(len(requests)),'name':name.split('.')[-1],'namespace':name.split('.')[0],'arguments':json.dumps(arguments),'status':'completed'} if name else {'id':ident,'type':'message','role':'assistant','status':'completed','content':[{'type':'output_text','text':text,'annotations':[]}]}
  response={'id':rid,'object':'response','status':'completed','output':[item],'usage':{'input_tokens':100,'output_tokens':20,'total_tokens':120}}
  added={**item,'arguments':''} if name else {**item,'content':[]}
  delta={'type':'response.function_call_arguments.delta','item_id':ident,'output_index':0,'delta':json.dumps(arguments)} if name else {'type':'response.output_text.delta','item_id':ident,'output_index':0,'content_index':0,'delta':text}
  events=[{'type':'response.created','response':{**response,'status':'in_progress','output':[]}},{'type':'response.output_item.added','output_index':0,'item':added},delta,{'type':'response.output_item.done','output_index':0,'item':item},{'type':'response.completed','response':response}]
  self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
  try:
   for event in events:self.wfile.write(('data: '+json.dumps(event)+'\n\n').encode())
   self.wfile.flush()
  except (BrokenPipeError,ConnectionResetError):pass
server=ThreadingHTTPServer(('127.0.0.1',0),Model);threading.Thread(target=server.serve_forever,daemon=True).start()
(root/'config.toml').write_text(f'''model="probe"
model_provider="probe"
approval_policy="on-request"
sandbox_mode="workspace-write"
[model_providers.probe]
name="Isolated model"
base_url="http://127.0.0.1:{server.server_port}/v1"
wire_api="responses"
requires_openai_auth=false
[mcp_servers.brevduva]
command={json.dumps(str(brv))}
args={json.dumps(['mcp','--config',str(config),'--binding','test/a@c'])}
[mcp_servers.brevduva.env]
BREVDUVA_TOKEN="isolated-fake-token"
[projects.{json.dumps(str(work))}]
trust_level="trusted"
''')
env=dict(os.environ,CODEX_HOME=str(root),TERM='xterm-256color')
for key in ['OPENAI_API_KEY','CODEX_THREAD_ID','BREVDUVA_BINDING','BREVDUVA_CONFIG','BREVDUVA_TOKEN']:env.pop(key,None)
exe=args.codex
approved_tools=set()
tui=PtyProcess.spawn([exe,'--no-alt-screen','ACTIVATE_NATIVE_931: enable automatic receiving and keep PRIVATE_CODEX_CONTEXT_753.'],cwd=str(work),env=env,dimensions=(40,130))

def read():
 try:
  while tui.isalive():
   c=tui.read(16384);terminal.append(c)
   clean=re.sub(r'\x1b\[[0-9;?]*[ -/]*[@-~]','', ''.join(terminal))
   for toolname in re.findall(r'Allow the brevduva MCP server to run tool \"([^\"]+)\"',clean):
    # 이 fixture의 세 가지 모의 MCP 도구에 한해서 현재 세션 승인을 재현한다.
    if toolname in {'receiver_connect','receipt','reply'} and toolname not in approved_tools and 'Allow for this session' in clean:
     approved_tools.add(toolname);time.sleep(.5);tui.write('2');time.sleep(.2);tui.write('\r')
   if '\x1b[6n' in c:tui.write('\x1b[1;1R')
   if '\x1b[c' in c:tui.write('\x1b[?1;2c')
 except (OSError,EOFError):pass
threading.Thread(target=read,daemon=True).start()
try:
 end=time.monotonic()+40
 while phase not in ['armed','done'] and time.monotonic()<end:time.sleep(.2)
 print('phase before event:',phase,'requests:',len(requests),'root:',root,flush=True)
 if phase=='armed':trigger.set();replied.wait(25);time.sleep(2)
 print(json.dumps({'approved_fixture_tools':sorted(approved_tools),'phase':phase,'reply':replied.is_set(),'frames':[x.get('op',x.get('error')) for x in frames],'root':str(root)}),flush=True)
finally:
 tui.close(force=True);server.shutdown();relayserver.shutdown();(root/'terminal.log').write_text(''.join(terminal),encoding='utf8');(root/'requests.json').write_text(json.dumps(requests),encoding='utf8');(root/'frames.json').write_text(json.dumps(frames),encoding='utf8')

assert replied.is_set() and phase == 'done', 'native Codex did not complete receipt and reply'
assert sum(f.get('op') == 'JOIN' for f in frames) == 1
assert any(f.get('op') == 'ACK' for f in frames)
assert any('PRIVATE_CODEX_CONTEXT_753' in json.dumps(r) and 'CODEX_NATIVE_PEER_492' in json.dumps(r) for r in requests)
