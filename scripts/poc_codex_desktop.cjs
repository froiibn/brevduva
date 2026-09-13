// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0
// Experimental installed Desktop IPC; target only an explicitly selected task.
const net = require('node:net'), fs = require('node:fs'), crypto = require('node:crypto');
function connect(path = '\\\\.\\pipe\\codex-ipc', timeoutMs = 20000) {
  const socket = net.connect(path), pending = new Map();
  let buffer = Buffer.alloc(0), clientId, closed = false;
  function fail(error) {
    closed = true;
    for (const p of pending.values()) { clearTimeout(p.timer); p.reject(error); }
    pending.clear();
  }
  socket.on('error', fail);
  socket.on('close', () => fail(Error('IPC disconnected; outcome unknown')));
  socket.on('data', part => {
    buffer = Buffer.concat([buffer, part]);
    while (buffer.length >= 4) {
      const size = buffer.readUInt32LE(0);
      if (size > 32 * 1024 * 1024) return socket.destroy(Error('IPC frame too large'));
      if (buffer.length < size + 4) break;
      let msg;
      try { msg = JSON.parse(buffer.subarray(4, size + 4)); }
      catch { return socket.destroy(Error('Invalid IPC JSON')); }
      buffer = buffer.subarray(size + 4);
      const p = pending.get(msg.requestId);
      if (msg.type !== 'response' || !p) continue;
      pending.delete(msg.requestId); clearTimeout(p.timer);
      if (msg.resultType === 'success') p.resolve(msg);
      else p.reject(Object.assign(Error(JSON.stringify(msg)), { explicitResponse: true }));
    }
  });
  function request(method, params, version, targetClientId) {
    return new Promise((resolve, reject) => {
      if (closed) return reject(Error('IPC unavailable'));
      const requestId = crypto.randomUUID();
      const timer = setTimeout(() => { pending.delete(requestId); reject(Error(`${method}: timeout, outcome unknown`)); }, timeoutMs + 1000);
      pending.set(requestId, { resolve, reject, timer });
      const body = Buffer.from(JSON.stringify({ type: 'request', requestId, sourceClientId: clientId ?? 'initializing-client',
        version, method, params, ...(targetClientId ? { targetClientId } : {}), timeoutMs }));
      const head = Buffer.alloc(4); head.writeUInt32LE(body.length); socket.write(Buffer.concat([head, body]));
    });
  }
  return { async initialize() { clientId = (await request('initialize', { clientType: 'brevduva-poc' }, 0)).result.clientId; },
    request, close() { socket.destroy(); } };
}
function turnParams(threadId, text) {
  const context = { version: 1, message: { source: 'mcp_app', sourceId: 'brevduva-delivery-poc', text } };
  const prompt = 'Respond to the user input in the context of our conversation.', callId = `brv_poc_${crypto.randomUUID()}`;
  return { conversationId: threadId, turnStart: {
    request: { threadId, input: [{ type: 'text', text: prompt, text_elements: [{
      byteRange: { start: 0, end: Buffer.byteLength(prompt) }, placeholder: 'codex-untrusted-app-input:' + JSON.stringify(context),
    }] }] },
    context: { inheritThreadSettings: true, responseItems: [
      { type: 'function_call', call_id: callId, name: 'untrusted_input', arguments: '{}' },
      { type: 'function_call_output', call_id: callId, output: [{ type: 'input_text', text: JSON.stringify({ kind: 'message', ...context.message }) }] },
    ] },
  } };
}
async function deliver(threadId, text, ipc = connect()) {
  try {
    await ipc.initialize();
    const owner = await ipc.request('thread-owner-discovery', { hostId: 'local', conversationId: threadId }, 1);
    if (!owner.handledByClientId || !owner.result.supportsUntrustedAppInput) throw Error('Selected owner does not support external app input');
    return await ipc.request('thread-follower-start-turn', turnParams(threadId, text), 2, owner.handledByClientId);
  } finally { ipc.close(); }
}
function isBusy(error) {
  return error.explicitResponse === true && error.message.includes('App context must wait until the current turn finishes');
}
if (require.main === module) (async () => {
  const [threadId, action, argument] = process.argv.slice(2);
  if (!threadId || !['discover', 'history', 'send'].includes(action)) throw Error('Usage: node poc_codex_desktop.cjs THREAD discover|history|send [message-file]');
  if (action === 'send') return console.log(JSON.stringify(await deliver(threadId, fs.readFileSync(argument, 'utf8'))));
  const ipc = connect();
  try {
    await ipc.initialize();
    const owner = await ipc.request('thread-owner-discovery', { hostId: 'local', conversationId: threadId }, 1);
    console.log(JSON.stringify(owner));
    if (action === 'history') console.log(JSON.stringify(await ipc.request('thread-follower-load-complete-history', { conversationId: threadId }, 1, owner.handledByClientId)));
  } finally { ipc.close(); }
})().catch(error => { console.error(error.message); process.exitCode = 1; });
module.exports = { connect, turnParams, deliver, isBusy };
