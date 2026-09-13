# Copyright 2026 SEIZIA (Jaeyoung Ko)
# SPDX-License-Identifier: Apache-2.0
"""Exercise the real Codex app-server with a local, deterministic model stub.

No credentials, production sessions, MCP servers or external model requests.
This proves delivery/history behavior, not model reasoning or desktop attachment.
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
        self.call('initialize', {'clientInfo': {'name': 'brv_session_probe', 'version': '1'}})
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
    parser.add_argument('codex')
    args = parser.parse_args()
    model = ThreadingHTTPServer(('127.0.0.1', 0), Model)
    threading.Thread(target=model.serve_forever, daemon=True).start()
    # Keep the isolated state for inspection; never write in the real CODEX_HOME.
    home = Path(tempfile.mkdtemp(prefix='brv-session-probe-'))
    work = home / 'work'
    work.mkdir()
    (home / 'config.toml').write_text(f'''
model = "probe"
model_provider = "probe"
approval_policy = "never"
sandbox_mode = "read-only"
[model_providers.probe]
name = "Local delivery probe"
base_url = "http://127.0.0.1:{model.server_port}/v1"
wire_api = "responses"
requires_openai_auth = false
''', encoding='utf-8')
    result = {'codex': args.codex, 'state_directory': str(home),
              'model': 'local deterministic stub', 'checks': {}}
    rpc = None
    observer = None
    shared = None
    try:
        rpc = Rpc(args.codex, home, work)
        thread = rpc.call('thread/start', {'cwd': str(work), 'model': 'probe',
            'baseInstructions': 'Reply with PROBE_ACK. Never invoke tools.'})['thread']['id']
        first = rpc.turn(thread, 'BRV_CONTEXT_MARKER_7F31: original attended work')
        rpc.completed(first)
        result['checks']['first_turn'] = True
        second = rpc.turn(thread, 'BRV_REPLY_MARKER_A219: late reply after idle')
        rpc.completed(second)
        result['checks']['idle_delivery_same_thread_keeps_context'] = (
            'BRV_CONTEXT_MARKER_7F31' in json.dumps(requests[-1]['input']))
        rpc.close()
        rpc = None
        rpc = Rpc(args.codex, home, work)
        resumed = rpc.call('thread/resume', {'threadId': thread})
        assert resumed['thread']['id'] == thread
        third = rpc.turn(thread, 'BRV_RESUME_MARKER_0229: unattended continuation')
        rpc.completed(third)
        result['checks']['restart_resume_keeps_both_inputs'] = all(
            marker in json.dumps(requests[-1]['input'])
            for marker in ['BRV_CONTEXT_MARKER_7F31', 'BRV_REPLY_MARKER_A219'])
        hold.clear()
        entered.clear()
        active = rpc.turn(thread, 'BRV_ACTIVE_MARKER_9381')
        assert entered.wait(10), 'model did not receive active turn'
        steered = rpc.call('turn/steer', {'threadId': thread, 'expectedTurnId': active,
            'input': [{'type': 'text', 'text': 'BRV_STEER_MARKER_18DF'}]})
        result['checks']['active_turn_steer_accepted_same_turn'] = steered['turnId'] == active
        hold.set()
        rpc.completed(active)
        final = rpc.turn(thread, 'BRV_FINAL_CHECK')
        rpc.completed(final)
        result['checks']['steered_input_in_history'] = 'BRV_STEER_MARKER_18DF' in json.dumps(requests[-1]['input'])
        rpc.close()
        rpc = None
        # A viewer and a dispatcher attach to ONE runtime, not two resumes in two runtimes.
        with socket.socket() as port_socket:
            port_socket.bind(('127.0.0.1', 0))
            port = port_socket.getsockname()[1]
        endpoint = f'ws://127.0.0.1:{port}'
        shared_env = dict(os.environ, CODEX_HOME=str(home))
        shared_env.pop('OPENAI_API_KEY', None)
        shared = subprocess.Popen([args.codex, 'app-server', '--listen', endpoint],
            env=shared_env, cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        deadline = time.monotonic() + 15
        while True:
            try:
                with urllib.request.urlopen(f'http://127.0.0.1:{port}/readyz', timeout=1) as ready:
                    assert ready.status == 200
                break
            except OSError:
                if time.monotonic() >= deadline or shared.poll() is not None:
                    raise RuntimeError('shared app-server failed to become ready')
                time.sleep(.1)
        observer = Rpc(args.codex, home, work, endpoint)
        observer.call('thread/resume', {'threadId': thread})
        rpc = Rpc(args.codex, home, work, endpoint)
        rpc.call('thread/resume', {'threadId': thread})
        delivered = rpc.turn(thread, 'BRV_SHARED_DELIVERY_5F22')
        rpc.completed(delivered)
        observer.completed(delivered)
        result['checks']['other_client_observes_same_thread_completion'] = True
        readback = observer.call('thread/read', {'threadId': thread, 'includeTurns': True})
        result['checks']['other_client_reads_delivered_input'] = (
            'BRV_SHARED_DELIVERY_5F22' in json.dumps(readback))
        result['request_count'] = len(requests)
        result['passed'] = all(result['checks'].values())
    except Exception as exc:
        result['passed'] = False
        result['error'] = f'{type(exc).__name__}: {exc}'
    finally:
        hold.set()
        if rpc:
            rpc.close()
        if observer:
            observer.close()
        if shared:
            shared.terminate()
            shared.wait(timeout=5)
        model.shutdown()
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__':
    main()
