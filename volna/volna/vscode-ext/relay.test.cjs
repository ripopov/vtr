const { test } = require('node:test');
const assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const { PassThrough, Writable } = require('node:stream');
const { Relay, LIMITS } = require('./relay');
const turn = () => new Promise(resolve => setImmediate(resolve));
async function settle() { for (let i = 0; i < 160; i++) await turn(); }
function frame(bytes) {
  const header = Buffer.alloc(4); header.writeUInt32LE(bytes.length);
  return Buffer.concat([header, bytes]);
}
function fixture(t, options = {}) {
  const child = new EventEmitter();
  child.stdin = options.stdin || new PassThrough();
  child.stdout = new PassThrough(); child.stderr = new PassThrough();
  child.kill = signal => { child.emit('close', null, signal); return true; };
  const messages = [];
  const relay = new Relay({ binary: '/bin/vtr-server', tracePath: '/tmp/test.vtr',
    spawn: () => child, postMessage: message => { messages.push(message); return options.post?.(message) ?? true; },
    ...options, stdin: undefined, post: undefined });
  t.after(async () => { const stopped = relay.dispose(); child.emit('close', 0, null); await stopped; });
  const consume = token => relay.receive({ type: 'rpcConsumed', incarnation: relay.incarnation, token });
  return { child, relay, messages, consume };
}
test('fragmented and coalesced frames retain exact opaque ArrayBuffers', async t => {
  const { child, messages } = fixture(t);
  const payloads = [Buffer.from([255, 0, 127]), Buffer.alloc(80000, 19), Buffer.from([1])];
  const bytes = Buffer.concat(payloads.map(frame));
  for (let offset = 0; offset < bytes.length; offset += 113) child.stdout.write(bytes.subarray(offset, offset + 113));
  await settle();
  const replies = messages.filter(m => m.type === 'rpcReply');
  assert.equal(replies.length, 3);
  replies.forEach((reply, i) => {
    assert.ok(reply.bytes instanceof ArrayBuffer);
    assert.deepEqual(Buffer.from(reply.bytes), payloads[i]);
    assert.equal(reply.token, String(i + 1));
  });
});
test('a frame header at the event-loop budget boundary does not stall', async t => {
  const { child, messages } = fixture(t);
  child.stdout.write(Buffer.concat([frame(Buffer.alloc(LIMITS.readTurn - 8)), frame(Buffer.from([42]))]));
  await settle();
  assert.equal(messages.filter(m => m.type === 'rpcReply').length, 2);
});
test('reply slots resume only after both posting and explicit consumption', async t => {
  let resolveFirst;
  const { child, relay, messages, consume } = fixture(t, { post: message =>
    message.type === 'rpcReply' && message.token === '1' ? new Promise(resolve => { resolveFirst = resolve; }) : true });
  child.stdout.write(Buffer.concat(Array.from({ length: 13 }, () => frame(Buffer.from([7])))));
  await settle();
  assert.equal(messages.length, 12);
  consume('1'); await settle(); assert.equal(messages.length, 12);
  resolveFirst(true); await settle(); assert.equal(messages.length, 13);
  assert.equal(relay.stats().deliveries, 12);
  consume('1'); assert.equal(relay.stats().deliveries, 12);
});
test('byte quota stops large replies before allocation', async t => {
  const { child, relay, messages, consume } = fixture(t);
  child.stdout.write(Buffer.concat(Array.from({ length: 5 }, () => frame(Buffer.alloc(LIMITS.reply, 8)))));
  await settle();
  assert.equal(messages.length, 4);
  assert.equal(relay.stats().reservedBytes, 4 * LIMITS.reply);
  consume('1'); await settle();
  assert.equal(messages.length, 5);
  assert.equal(relay.stats().reservedBytes, 4 * LIMITS.reply);
});
test('hidden view pauses pumping; obsolete incarnation cannot send or acknowledge', async t => {
  const { child, relay, messages } = fixture(t, { visible: false });
  child.stdout.write(frame(Buffer.from([3]))); await settle(); assert.equal(messages.length, 0);
  assert.equal(relay.receive({ type: 'rpcSend', incarnation: 'old' }), false);
  assert.equal(relay.failure, undefined);
  relay.setVisible(true); await settle(); assert.equal(messages.length, 1);
});
test('stdin credit lasts through actual write completion', async t => {
  const writes = []; let release;
  const stdin = new Writable({ write(chunk, encoding, callback) {
    writes.push(Buffer.from(chunk)); if (writes.length === 2) release = callback; else callback();
  } });
  const { relay, messages } = fixture(t, { stdin });
  const send = token => relay.receive({ type: 'rpcSend', incarnation: relay.incarnation, token, bytes: Uint8Array.of(8, 9).buffer });
  assert.equal(send('1'), true); await turn();
  assert.equal(messages.length, 0); assert.equal(relay.stats().sendBytes, 2);
  release(); await settle();
  assert.equal(messages[0].type, 'rpcWritten');
  assert.deepEqual(Buffer.concat(writes), frame(Buffer.from([8, 9])));
});
test('invalid lengths and truncated frames terminate the relay', async t => {
  for (const bytes of [Buffer.alloc(4), Buffer.from([1, 0]), Buffer.from([3, 0, 0, 0, 8])]) {
    const { child, relay, messages } = fixture(t);
    child.stdout.end(bytes); await settle();
    assert.ok(relay.failure); assert.equal(relay.stats().reservedBytes, 0);
    assert.ok(messages.some(m => m.type === 'rpcFailed'));
  }
});
test('disposal retains unresolved post reservations and bounds diagnostics', async t => {
  let resolvePost;
  const { child, relay } = fixture(t, { post: () => new Promise(resolve => { resolvePost = resolve; }) });
  child.stderr.write(Buffer.alloc(100000, 65));
  child.stdout.write(frame(Buffer.alloc(99))); await settle();
  assert.equal(relay.stats().diagnosticBytes, LIMITS.diagnostics);
  const stopped = relay.dispose(); child.emit('close', 0, null);
  assert.equal(relay.stats().reservedBytes, 99);
  resolvePost(true); await stopped;
  assert.equal(relay.stats().reservedBytes, 0);
});

test('real child exit preserves final frames before the stopped notification', async t => {
  const { spawn } = require('node:child_process');
  const messages = [];
  const relay = new Relay({ binary: '/bin/vtr-server', tracePath: '/tmp/test.vtr',
    spawn: () => spawn(process.execPath, ['-e',
      'const b=Buffer.alloc(80004,17);b.writeUInt32LE(80000);process.stdout.write(b);']),
    postMessage: message => { messages.push(message); return true; } });
  t.after(() => relay.dispose());
  await relay.stopped;
  assert.deepEqual(messages.map(m => m.type), ['rpcReply', 'rpcStopped']);
  assert.equal(messages[0].bytes.byteLength, 80000);
});
test('child close with unread hidden output explicitly invalidates the transport', async t => {
  const { child, relay, messages } = fixture(t, { visible: false });
  child.stdout.write(frame(Buffer.from([1])));
  child.emit('close', 0, null);
  await relay.stopped;
  assert.match(relay.failure, /unread/);
  assert.deepEqual(messages.map(m => m.type), ['rpcFailed']);
});
test('sender exceeding the single stdin packet credit is rejected', async t => {
  let release;
  const stdin = new Writable({ write(chunk, encoding, callback) { release = callback; } });
  const { relay } = fixture(t, { stdin });
  const send = token => relay.receive({ type: 'rpcSend', incarnation: relay.incarnation,
    token, bytes: Uint8Array.of(1).buffer });
  assert.equal(send('1'), true);
  assert.equal(send('2'), false);
  assert.match(relay.failure, /without relay credit/);
  release(); // Drain header, then payload, so disposal can finish.
  release();
});
test('unresponsive child receives termination then kill and disposal waits for close', async t => {
  const { child, relay } = fixture(t, { graceMs: 1, killMs: 5 });
  const signals = [];
  child.kill = signal => { signals.push(signal); if (signal === 'SIGKILL') child.emit('close', null, signal); return true; };
  // Keep the test loop alive; production timers deliberately do not pin Node.
  const keepAlive = setTimeout(() => {}, 1000);
  try { await relay.dispose(); } finally { clearTimeout(keepAlive); }
  assert.deepEqual(signals, ['SIGTERM', 'SIGKILL']);
  assert.equal(relay.stats().childClosed, true);
});
