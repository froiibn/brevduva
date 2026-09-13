"""Isolated real Claude process: initial turn, then channel-only idle wake.
No application code edits; no built-in tools, no existing sessions resumed.
"""
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import uuid

root = Path(tempfile.mkdtemp(prefix='brv-claude-channel-poc-'))
trigger = root / 'trigger.json'
server = root / 'channel.cjs'
server.write_text(r'''
const fs=require('fs'), readline=require('readline');
const send=x=>process.stdout.write(JSON.stringify(x)+'\n');
readline.createInterface({input:process.stdin}).on('close',()=>process.exit(0)).on('line',line=>{
 let r;try{r=JSON.parse(line)}catch{return}
 if(r.id===undefined)return;
 let result;
 if(r.method==='initialize')result={protocolVersion:'2025-06-18',capabilities:{tools:{},experimental:{'claude/channel':{}}},serverInfo:{name:'brv-poc',version:'0.0.1'},instructions:'This is an isolated delivery test. On a channel event, reply in the conversation with the event marker and the remembered token.'};
 else if(r.method==='tools/list')result={tools:[]};
 else if(r.method==='ping')result={};
 else return send({jsonrpc:'2.0',id:r.id,error:{code:-32601,message:'Not found'}});
 send({jsonrpc:'2.0',id:r.id,result});
});
let done=false;
setInterval(()=>{if(done||!fs.existsSync(process.argv[2]))return;
 done=true;const p=JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
 send({jsonrpc:'2.0',method:'notifications/claude/channel',params:p});
 fs.writeFileSync(process.argv[2]+'.sent','sent');
},100);
''', encoding='utf-8')
config = root / 'mcp.json'
config.write_text(json.dumps({'mcpServers': {'brv-poc': {'command':'node','args':[str(server),str(trigger)]}}}),encoding='utf-8')
session = str(uuid.uuid4())
token = 'MEMORY_' + uuid.uuid4().hex[:12]
marker = 'CHANNEL_' + uuid.uuid4().hex[:12]
exe = str(Path.home()/'.local/bin/claude.exe')
args = [exe,'-p','--verbose','--input-format','stream-json','--output-format','stream-json',
        '--no-session-persistence','--session-id',session,'--tools','',
        '--debug-file',str(root/'debug.log'),
        '--strict-mcp-config','--mcp-config',str(config),
        '--dangerously-load-development-channels','server:brv-poc',
        '--setting-sources','','--max-budget-usd','1',
        '--system-prompt','You are a delivery test. Do not perform actions. Remember the token from the initial message. For each channel event reply with its marker and the remembered token.']
if '--prepare-tui' in sys.argv:
    tui_args = [exe,'--session-id',session,'--tools','',
        '--debug-file',str(root/'debug.log'),'--strict-mcp-config','--mcp-config',str(config),
        '--dangerously-load-development-channels','server:brv-poc','--setting-sources','',
        '--system-prompt','You are a delivery test. Do not perform actions. Remember the token from the initial message. For each channel event reply with its marker and the remembered token.',
        f'Remember {token}. Reply READY only.']
    (root/'tui-args.json').write_text(json.dumps(tui_args),encoding='utf-8')
    (root/'test-data.json').write_text(json.dumps({'session_id':session,'memory_token':token,'marker':marker,'trigger_path':str(trigger)}),encoding='utf-8')
    print(root)
    raise SystemExit(0)
events = queue.Queue()
result = {'directory':str(root),'session_id':session,'checks':{},'transport':'Claude stream-json process + MCP channel','scope':'headless process, not interactive TUI or Desktop'}
log = open(root/'stderr.log','w',encoding='utf-8')
p = subprocess.Popen(args,cwd=root,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=log,text=True,encoding='utf-8',creationflags=subprocess.CREATE_NO_WINDOW if os.name=='nt' else 0)
def reader():
    for line in p.stdout:
        with open(root/'events.jsonl','a',encoding='utf-8') as event_log: event_log.write(line)
        try: events.put(json.loads(line))
        except ValueError: pass
threading.Thread(target=reader,daemon=True).start()
captured=[]
def until(predicate,seconds):
    end=time.monotonic()+seconds
    while time.monotonic()<end:
        try:e=events.get(timeout=.25)
        except queue.Empty:
            if p.poll() is not None:raise RuntimeError('Claude exited: '+str(p.returncode))
            continue
        captured.append(e)
        if predicate(e):return e
    raise TimeoutError('No matching output within '+str(seconds)+' seconds')
try:
    initial={'type':'user','session_id':session,'message':{'role':'user','content':f'Remember {token}. Reply READY only.'},'parent_tool_use_id':None}
    p.stdin.write(json.dumps(initial)+'\n');p.stdin.flush()
    first=until(lambda e:e.get('type')=='result',75)
    result['checks']['initial_turn_completed']=not first.get('is_error',False)
    if first.get('is_error'):raise RuntimeError(str(first.get('result') or first.get('errors')))
    # Keep the exact process alive and idle; do not send any second stdin message.
    time.sleep(2)
    result['checks']['same_process_alive_after_initial_turn']=p.poll() is None
    trigger.write_text(json.dumps({'content':f'Test event {marker}. Reply with this marker and the token you remember.','meta':{'message_id':marker}}),encoding='utf-8')
    second=until(lambda e:e.get('type')=='result',60)
    text=json.dumps(second,ensure_ascii=False)
    result['checks']['channel_caused_second_result']=not second.get('is_error',False)
    result['checks']['same_session']=second.get('session_id')==session
    result['checks']['marker_and_initial_memory_in_reply']=marker in text and token in text
except Exception as exc:
    result['error']=str(exc)
finally:
    result['channel_transport_wrote']=Path(str(trigger)+'.sent').exists()
    p.stdin.close()
    try:p.wait(timeout=8)
    except subprocess.TimeoutExpired:p.terminate();p.wait(timeout=5)
    log.close()
    (root/'events.json').write_text(json.dumps(captured,ensure_ascii=False,indent=2),encoding='utf-8')
    result['passed']=len(result['checks'])==5 and all(result['checks'].values())
    (root/'result.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
    print(json.dumps(result,indent=2),flush=True)
