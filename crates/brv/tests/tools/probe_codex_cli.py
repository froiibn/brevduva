# Copyright 2026 SEIZIA (Jaeyoung Ko)
# SPDX-License-Identifier: Apache-2.0
"""Exercise the real Codex app-server with a local, deterministic model stub.

No credentials, production sessions, MCP servers or external model requests.
This proves actual brv/app-server integration with a model stub, not real model reasoning.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

requests = []
hold = threading.Event()
entered = threading.Event()
hold.set()


class Model(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        requests.append(body)
        entered.set()
        hold.wait(30)
        msg = {'id': 'msg_probe', 'type': 'message', 'role': 'assistant',
               'status': 'completed', 'content': [
                   {'type': 'output_text', 'text': 'PROBE_ACK', 'annotations': []}]}
        response = {'id': 'resp_probe', 'object': 'response', 'status': 'completed',
                    'output': [msg], 'usage': {'input_tokens': 1, 'output_tokens': 1,
                    'total_tokens': 2}}
        events = [
            {'type': 'response.created', 'response': {**response, 'status': 'in_progress', 'output': []}},
            {'type': 'response.output_item.added', 'output_index': 0, 'item': {**msg, 'content': []}},
            {'type': 'response.output_text.delta', 'item_id': 'msg_probe', 'output_index': 0,
             'content_index': 0, 'delta': 'PROBE_ACK'},
            {'type': 'response.output_item.done', 'output_index': 0, 'item': msg},
            {'type': 'response.completed', 'response': response},
        ]
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        try:
            for event in events:
                self.wfile.write(('data: ' + json.dumps(event) + '\n\n').encode())
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


class Rpc:
    def __init__(self, exe, home, work, endpoint=None):
        env = dict(os.environ, CODEX_HOME=str(home))
        env.pop('OPENAI_API_KEY', None)
        self.log = open(home / 'probe-stderr.log', 'a', encoding='utf-8')
        command = [exe, 'app-server', '--listen', 'stdio://']
        if endpoint:
            # Node 22+ provides WebSocket; bridge JSONL without third-party packages.
            command = ['node', '-e', '''
const ws = new WebSocket(process.argv[1]);
ws.addEventListener('open', () => {
  const lines = require('node:readline').createInterface({input: process.stdin});
  lines.on('line', line => ws.send(line));
  lines.on('close', () => ws.close());
});
ws.addEventListener('message', event => process.stdout.write(event.data + '\\n'));
ws.addEventListener('error', () => process.exit(1));
ws.addEventListener('close', () => process.exit(0));
''', endpoint]
        self.proc = subprocess.Popen(command,
            cwd=work, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=self.log, text=True, encoding='utf-8',
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        self.events = queue.Queue()
        self.backlog = []
        self.seq = 0
        def reader():
            for line in self.proc.stdout:
                try:
                    self.events.put(json.loads(line))
                except json.JSONDecodeError:
                    pass
            self.events.put({'eof': True})
        threading.Thread(target=reader, daemon=True).start()
        self.call('initialize', {'clientInfo': {'name': 'brv_session_probe', 'version': '1'}, 'capabilities': {'experimentalApi': True}})
        self.send({'method': 'initialized'})

    def send(self, value):
        self.proc.stdin.write(json.dumps(value) + '\n')
        self.proc.stdin.flush()

    def wait(self, predicate, seconds=35):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            for i, event in enumerate(self.backlog):
                if predicate(event):
                    return self.backlog.pop(i)
            event = self.events.get(timeout=max(.01, deadline - time.monotonic()))
            if event.get('eof'):
                raise RuntimeError('app-server exited; inspect probe-stderr.log')
            if 'method' in event and 'id' in event:
                self.send({'id': event['id'], 'error': {'code': -32601, 'message': 'Probe permits no tools'}})
            elif predicate(event):
                return event
            else:
                self.backlog.append(event)
        raise TimeoutError('RPC/event timed out')

    def call(self, method, params):
        self.seq += 1
        request_id = self.seq
        self.send({'id': request_id, 'method': method, 'params': params})
        reply = self.wait(lambda e: e.get('id') == request_id)
        if 'error' in reply:
            raise RuntimeError(f'{method}: {reply["error"]}')
        return reply['result']

    def turn(self, thread, text):
        result = self.call('turn/start', {'threadId': thread,
            'input': [{'type': 'text', 'text': text}]})
        return result['turn']['id']

    def completed(self, turn):
        event = self.wait(lambda e: e.get('method') == 'turn/completed'
            and e['params']['turn']['id'] == turn)
        assert event['params']['turn']['status'] == 'completed', event

    def close(self):
        if self.log.closed:
            return
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.terminate()
            self.proc.wait(timeout=5)
        self.log.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--codex', required=True)
    parser.add_argument('--brv', required=True)
    parser.add_argument('--deps', help='Optional directory containing websockets and pywinpty')
    parser.add_argument('--tui', action='store_true', help='Also observe the real Windows TUI in a pseudoterminal')
    args = parser.parse_args()
    import sys
    if args.deps:
        sys.path.insert(0, str(Path(args.deps).resolve()))
    from websockets.sync.server import serve

    home = Path(tempfile.mkdtemp(prefix='brv-codex-integration-'))
    work = home / 'work'
    work.mkdir()
    model = ThreadingHTTPServer(('127.0.0.1', 0), Model)
    threading.Thread(target=model.serve_forever, daemon=True).start()
    message_id = '01ARZ3NDEKTSV4RRFFQ69G5FAV'
    published = threading.Event()
    acked = threading.Event()
    joins = []
    failures = []
    frames = []

    def relay(ws):
        try:
            join = json.loads(ws.recv(timeout=10))
            assert join['op'] == 'JOIN', join
            joins.append(join)
            ws.send(json.dumps({'op': 'OK', 're': join['seq'], 'body': {}}))
            ws.send(json.dumps({'op': 'DELIVER', 'seq': 700, 'body': {
                'v': 1, 'id': message_id, 'client_key': message_id,
                'from': 'peer', 'to': 'agent:a', 'kind': 'request', 'expects': 'reply',
                'hops': 2, 'content_type': 'text/plain', 'payload': 'ISOLATED_PEER_REQUEST', 'meta': {}}}))
            for raw in ws:
                frame = json.loads(raw)
                frames.append(frame)
                if frame['op'] == 'ACK':
                    journal = next((home / 'brv').rglob('deliveries.jsonl'))
                    assert message_id in journal.read_text(encoding='utf8')
                    acked.set()
                elif frame['op'] == 'PUB':
                    assert acked.is_set()
                    assert frame['body']['correlation_id'] == message_id
                    assert frame['body']['hops'] == 3
                    ws.send(json.dumps({'op': 'OK', 're': frame['seq'], 'body': {'id': '01ARZ3NDEKTSV4RRFFQ69G5FAW'}}))
                    published.set()
                elif frame['op'] == 'PING':
                    ws.send(json.dumps({'op': 'PONG', 're': frame['seq']}))
        except Exception as error:
            if not published.is_set():
                failures.append(repr(error))

    relay_server = serve(relay, '127.0.0.1', 0)
    threading.Thread(target=relay_server.serve_forever, daemon=True).start()
    relay_port = relay_server.socket.getsockname()[1]
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    endpoint = f'ws://127.0.0.1:{port}'
    config_dir = home / 'brv'
    config_dir.mkdir()
    config = config_dir / 'config.toml'
    config.write_text(f'server="http://127.0.0.1:{relay_port}"\n[[binding]]\norg="test"\nagent="a"\nchannel="c"\n', encoding='utf8')
    entry_args = ['mcp', '--config', str(config), '--binding', 'test/a@c', '--codex-cli-endpoint', endpoint]
    (home / 'config.toml').write_text(f'''
model = "probe"
model_provider = "probe"
approval_policy = "never"
sandbox_mode = "read-only"
[model_providers.probe]
name = "Local deterministic integration probe"
base_url = "http://127.0.0.1:{model.server_port}/v1"
wire_api = "responses"
requires_openai_auth = false
[mcp_servers.brevduva]
command = {json.dumps(str(Path(args.brv).resolve()))}
args = {json.dumps(entry_args)}
[mcp_servers.brevduva.env]
BREVDUVA_TOKEN = "isolated-fake-token"
''', encoding='utf8')
    env = dict(os.environ, CODEX_HOME=str(home), TERM='xterm-256color')
    for key in ['OPENAI_API_KEY', 'CODEX_THREAD_ID', 'BREVDUVA_TOKEN', 'BREVDUVA_CONFIG', 'BREVDUVA_BINDING']:
        env.pop(key, None)
    server = subprocess.Popen([args.codex, 'app-server', '--listen', endpoint], cwd=work, env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
    rpc = tui = None
    terminal = []
    result = {'state_directory': str(home), 'checks': {}}
    try:
        deadline = time.monotonic() + 10
        while True:
            try:
                with urllib.request.urlopen(f'http://127.0.0.1:{port}/readyz', timeout=1):
                    break
            except OSError:
                if time.monotonic() > deadline:
                    raise
                time.sleep(.1)
        rpc = Rpc(args.codex, home, work, endpoint)
        tid = rpc.call('thread/start', {'cwd': str(work), 'model': 'probe'})['thread']['id']
        rpc.completed(rpc.turn(tid, 'PRIVATE_CONTEXT_ONLY_72'))
        if args.tui:
            from winpty import PtyProcess
            tui = PtyProcess.spawn([args.codex, '--remote', endpoint, 'resume', tid, '--no-alt-screen'],
                cwd=str(work), env=env, dimensions=(35, 120))
            def read_terminal():
                try:
                    while tui.isalive():
                        chunk = tui.read(8192)
                        terminal.append(chunk)
                        if '\x1b[6n' in chunk:
                            tui.write('\x1b[1;1R')
                        if '\x1b[c' in chunk:
                            tui.write('\x1b[?1;2c')
                except (OSError, EOFError):
                    pass
            threading.Thread(target=read_terminal, daemon=True).start()
            time.sleep(4)
            assert 'PRIVATE_CONTEXT_ONLY_72' in ''.join(terminal), 'TUI did not load the target history'
        def tool(name, arguments):
            value = rpc.call('mcpServer/tool/call', {'threadId': tid, 'server': 'brevduva', 'tool': name, 'arguments': arguments})
            assert not value.get('isError'), value
            return json.loads(value['content'][0]['text'])
        initial = tool('receiver_session_status', {})
        assert not initial['delivery']['automatic_delivery']
        assert not joins, 'unbound MCP joined prematurely'
        tool('receiver_connect', {'thread_id': tid})
        rpc.wait(lambda event: event.get('method') == 'turn/completed' and event['params']['threadId'] == tid)
        serialized = json.dumps(requests[-1]['input'])
        outputs = [item for item in requests[-1]['input'] if item.get('name') == 'brevduva_message']
        assert outputs and outputs[-1]['type'] == 'function_call_output'
        payload = json.loads(outputs[-1]['output'])
        assert 'PRIVATE_CONTEXT_ONLY_72' in serialized
        assert 'ISOLATED_PEER_REQUEST' in serialized
        result['checks']['automatic_turn_same_thread_with_tool_output_and_context'] = True
        # This is a mock host receipt, not proof that a real model called the tool.
        tool('receipt', payload['meta'])
        tool('reply', {'to': 'peer', 'correlation_id': message_id, 'payload': 'isolated reply'})
        assert published.wait(5), failures
        assert len(joins) == 1 and not failures, (joins, failures)
        result['checks']['durable_ack_receipt_and_reply_one_connection'] = True
        if tui:
            time.sleep(1)
            assert ''.join(terminal).count('PROBE_ACK') >= 2, 'TUI did not render both model turns'
            result['checks']['real_tui_observed_model_output'] = True
        result['passed'] = True
    except Exception as error:
        result['passed'] = False
        result['error'] = repr(error)
    finally:
        if tui:
            tui.close(force=True)
        if rpc:
            rpc.close()
        server.terminate()
        server.wait(timeout=5)
        relay_server.shutdown()
        model.shutdown()
        (home / 'terminal.log').write_text(''.join(terminal), encoding='utf8')
        (home / 'model-requests.json').write_text(json.dumps(requests), encoding='utf8')
        (home / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf8')
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__':
    main()
