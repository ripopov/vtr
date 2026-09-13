// Real native endpoint integration. check.sh supplies the built binary path.
// Fixed wire fixtures here exercise the opaque courier, not a JavaScript codec.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { Relay } = require('./relay');
test('native server handshake and malformed request travel through the relay', { timeout: 10000 }, async t => {
  assert.ok(process.env.VTR_SERVER, 'VTR_SERVER must name the built native query child');
  const messages = []; const waiters = [];
  const relay = new Relay({ binary: path.resolve(process.env.VTR_SERVER),
    tracePath: path.resolve(__dirname, '../examples/counter.vtr'),
    postMessage: message => {
      const waiter = waiters.shift(); if (waiter) waiter(message); else messages.push(message);
      return true;
    } });
  t.after(() => relay.dispose());
  const next = () => messages.length ? Promise.resolve(messages.shift()) : new Promise(resolve => waiters.push(resolve));
  const send = (token, bytes) => assert.equal(relay.receive({ type: 'rpcSend',
    incarnation: relay.incarnation, token, bytes: Uint8Array.from(bytes).buffer }), true);
  send('1', [8, 1, 16, 1, 82, 0]); // version=1, request_id=1, Hello
  let response;
  for (let i = 0; i < 2; i++) {
    const message = await next();
    if (message.type === 'rpcReply') response = message;
    else assert.equal(message.type, 'rpcWritten');
  }
  assert.ok(response);
  const bytes = Buffer.from(response.bytes);
  assert.deepEqual([...bytes.subarray(0, 5)], [8, 1, 16, 1, 90]); // Welcome field
  relay.receive({ type: 'rpcConsumed', incarnation: relay.incarnation, token: response.token });
  // Unsupported field is a terminal protocol error; diagnostics use stderr.
  send('2', [8, 1, 16, 2, 250, 7, 0]);
  await relay.stopped;
  const terminal = messages.find(m => m.type === 'rpcStopped');
  assert.ok(terminal);
  assert.notEqual(terminal.code, 0);
  assert.equal(messages.filter(m => m.type === 'rpcReply').length, 0);
  assert.equal(relay.stats().reservedBytes, 0);
});
