# Copyright 2026 SEIZIA (Jaeyoung Ko)
# SPDX-License-Identifier: Apache-2.0
"""Windows 실제 Claude TUI + 로컬 모의 모델/WS. Monitor 제공 환경을 재현한다.
프로덕션 설정·토큰·세션을 변경하지 않는다. API 모델의 추론 품질 시험은 아니다.
"""
import argparse, json, os, re, sys, tempfile, threading, time
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
sys.dont_write_bytecode = True
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--brv', default='target/debug/brv.exe')
parser.add_argument('--claude', default=str(Path.home()/'.local/bin/claude.exe'))
parser.add_argument('--deps', default='target/cli-probe-deps')
args = parser.parse_args()
sys.path.insert(0,str(Path(args.deps).resolve()))
from winpty import PtyProcess
from websockets.sync.server import serve
root=Path(tempfile.mkdtemp(prefix='brv-claude-native-'));work=root/'work';work.mkdir();cfgdir=root/'claude';cfgdir.mkdir()
mid='01ARZ3NDEKTSV4RRFFQ69G5FAV';trigger=threading.Event();replied=threading.Event();requests=[];frames=[];phase='start';terminal=[]
def relay(ws):
 try:
  join=json.loads(ws.recv(timeout=40));frames.append(join);ws.send(json.dumps({'op':'OK','re':join['seq'],'body':{}}))
  assert trigger.wait(40)
  ws.send(json.dumps({'op':'DELIVER','seq':700,'body':{'v':1,'id':mid,'client_key':mid,'from':'peer','to':'agent:a','kind':'request','expects':'reply','hops':2,'content_type':'text/plain','payload':'CLAUDE_NATIVE_PEER_492','meta':{}}}))
  for raw in ws:
   f=json.loads(raw);frames.append(f)
   if f['op']=='PUB':
    assert f['body']['correlation_id']==mid and f['body']['hops']==3 and f['body']['payload']=='CLAUDE_NATIVE_REPLY',f
    ws.send(json.dumps({'op':'OK','re':f['seq'],'body':{'id':'01ARZ3NDEKTSV4RRFFQ69G5FAW'}}));replied.set()
   elif f['op']=='PING':ws.send(json.dumps({'op':'PONG','re':f['seq']}))
 except Exception as e:frames.append({'error':repr(e)})
relayserver=serve(relay,'127.0.0.1',0);threading.Thread(target=relayserver.serve_forever,daemon=True).start()
brvdir=root/'brv';brvdir.mkdir();config=brvdir/'config.toml';config.write_text(f'server="http://127.0.0.1:{relayserver.socket.getsockname()[1]}"\n[[binding]]\norg="test"\nagent="a"\nchannel="c"\n')
brv=Path(args.brv).resolve()
mcp=work/'.mcp.json';mcp.write_text(json.dumps({'mcpServers':{'brevduva':{'command':str(brv),'args':['mcp','--config',str(config),'--binding','test/a@c'],'env':{'BREVDUVA_TOKEN':'isolated-fake-token'}}}}))
# 모의 API는 원격 기능 플래그를 제공하지 않으므로 Monitor가 제공되는 환경을 fixture에 명시한다.
(cfgdir/'.claude.json').write_text(json.dumps({'hasCompletedOnboarding':True,'cachedGrowthBookFeatures':{'tengu_amber_sentinel':True},'theme':'dark','projects':{work.as_posix():{'hasTrustDialogAccepted':True,'enabledMcpjsonServers':['brevduva']}}}))
(cfgdir/'settings.json').write_text(json.dumps({'permissions':{'allow':['Monitor','mcp__brevduva__*']},'enableAllProjectMcpServers':True}))
def strings(v):
 if isinstance(v,str):yield v
 elif isinstance(v,list):
  for x in v:yield from strings(x)
 elif isinstance(v,dict):
  for x in v.values():yield from strings(x)
class Model(BaseHTTPRequestHandler):
 def log_message(self,*a):pass
 def do_GET(self):self.send_response(200);self.end_headers();self.wfile.write(b'{}')
 def do_POST(self):
  global phase
  r=json.loads(self.rfile.read(int(self.headers.get('Content-Length',0))) or b'{}');requests.append({'path':self.path,'body':r})
  if 'count_tokens' in self.path:
   self.send_response(200);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(b'{"input_tokens":100}');return
  names=[x.get('name','') for x in r.get('tools',[])];flat='\n'.join(strings(r.get('messages',[])));name=None;args={};text='READY_NATIVE'
  if 'ACTIVATE_NATIVE_931' in flat:
   connect=next((x for x in names if x.endswith('receiver_connect')),None)
   if phase=='start' and not connect and 'WaitForMcpServers' in names:
    name='WaitForMcpServers';args={}
   elif phase=='start' and connect:
    name=connect;args={'session_kind':'claude-cli','monitor_available':True};phase='connect'
   elif phase=='connect':
    for val in strings(r.get('messages',[])):
     try:
      x=json.loads(val)
      if isinstance(x,dict) and x.get('next_tool')=='Monitor':name='Monitor';args=x['arguments'];phase='monitor';break
     except ValueError:pass
   elif phase in ['monitor','armed']:
    phase='armed';text='ARMED_NATIVE'
    found=re.search(r'"receipt_token"\s*:\s*"([A-Z0-9]{26})"',flat)
    if found and any(x.endswith('receipt') for x in names):
     name=next(x for x in names if x.endswith('receipt'));args={'message_id':mid,'receipt_token':found.group(1)};phase='receipt'
   elif phase=='receipt' and 'CLAUDE_NATIVE_PEER_492' in flat:
    name=next(x for x in names if x.endswith('__reply'));args={'to':'peer','correlation_id':mid,'payload':'CLAUDE_NATIVE_REPLY'};phase='reply'
   elif phase=='reply':text='DELIVERED_NATIVE';phase='done'
  content={'type':'tool_use','id':'tool_'+str(len(requests)),'name':name,'input':args} if name else {'type':'text','text':text}
  stop='tool_use' if name else 'end_turn';msg={'id':'msg_'+str(len(requests)),'type':'message','role':'assistant','model':r.get('model','claude-sonnet-4-6'),'content':[content],'stop_reason':stop,'stop_sequence':None,'usage':{'input_tokens':100,'output_tokens':20}}
  self.send_response(200)
  if r.get('stream'):
   self.send_header('Content-Type','text/event-stream');self.end_headers()
   start={**content,'input':{}} if name else {'type':'text','text':''}
   delta={'type':'input_json_delta','partial_json':json.dumps(args)} if name else {'type':'text_delta','text':text}
   events=[('message_start',{'message':{**msg,'content':[],'stop_reason':None}}),('content_block_start',{'index':0,'content_block':start}),('content_block_delta',{'index':0,'delta':delta}),('content_block_stop',{'index':0}),('message_delta',{'delta':{'stop_reason':stop,'stop_sequence':None},'usage':{'output_tokens':20}}),('message_stop',{})]
   try:
    for typ,data in events:self.wfile.write(('event: '+typ+'\ndata: '+json.dumps({'type':typ,**data})+'\n\n').encode());self.wfile.flush()
   except (BrokenPipeError,ConnectionResetError):pass
  else:self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(json.dumps(msg).encode())
server=ThreadingHTTPServer(('127.0.0.1',0),Model);threading.Thread(target=server.serve_forever,daemon=True).start()
env=dict(os.environ,CLAUDE_CONFIG_DIR=str(cfgdir),ANTHROPIC_BASE_URL=f'http://127.0.0.1:{server.server_port}',ANTHROPIC_API_KEY='sk-ant-api03-isolated-probe-key',ENABLE_TOOL_SEARCH='false',TERM='xterm-256color')
for key in ['ANTHROPIC_AUTH_TOKEN','CLAUDE_CODE_OAUTH_TOKEN','CLAUDECODE','CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC','DISABLE_TELEMETRY','BREVDUVA_BINDING']:env.pop(key,None)
exe=args.claude
approved_workspace=False
approved_key=False
tui=PtyProcess.spawn([exe,'--model','claude-sonnet-4-6','ACTIVATE_NATIVE_931: enable automatic receiving and keep PRIVATE_CLAUDE_CONTEXT_753.'],cwd=str(work),env=env,dimensions=(40,130))
def read():
 global approved_workspace, approved_key
 try:
  while tui.isalive():
   c=tui.read(16384);terminal.append(c)
   compact=re.sub(r'\s+|\x1b\[[0-9;?]*[A-Za-z]','', ''.join(terminal))
   if not approved_key and 'DoyouwanttousethisAPIkey?' in compact:
    approved_key=True;time.sleep(2.5);tui.write('\x1b[A');time.sleep(.5);tui.write('\r')
   if not approved_workspace and 'Yes,Itrustthisfolder' in compact:
    approved_workspace=True;time.sleep(2.5);tui.write('\x1b[B');time.sleep(.5);tui.write('\r')
   if '\x1b[6n' in c:tui.write('\x1b[1;1R')
   if '\x1b[c' in c:tui.write('\x1b[?1;2c')
 except (OSError,EOFError):pass
threading.Thread(target=read,daemon=True).start()
try:
 end=time.monotonic()+40
 while phase not in ['armed','done'] and time.monotonic()<end:time.sleep(.2)
 print('phase before event:',phase,'requests:',len(requests),'root:',root,flush=True)
 if phase=='armed':trigger.set();replied.wait(25);time.sleep(2)
 print(json.dumps({'monitor_present':any(any(t.get('name')=='Monitor' for t in r['body'].get('tools',[])) for r in requests),'phase':phase,'reply':replied.is_set(),'frames':[x.get('op',x.get('error')) for x in frames],'root':str(root)}),flush=True)
finally:
 tui.close(force=True);server.shutdown();relayserver.shutdown();(root/'terminal.log').write_text(''.join(terminal),encoding='utf8');(root/'requests.json').write_text(json.dumps(requests),encoding='utf8');(root/'frames.json').write_text(json.dumps(frames),encoding='utf8')

assert replied.is_set() and phase == 'done', 'native Monitor did not complete receipt and reply'
assert sum(f.get('op') == 'JOIN' for f in frames) == 1
assert any(f.get('op') == 'ACK' for f in frames)
assert any('PRIVATE_CLAUDE_CONTEXT_753' in json.dumps(r) and 'CLAUDE_NATIVE_PEER_492' in json.dumps(r) for r in requests)
