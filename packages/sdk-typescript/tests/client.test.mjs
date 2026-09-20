import { test } from 'node:test';
import assert from 'node:assert/strict';
import { OpenForgeClient } from '../dist/client.js';

test('run listing sends authentication and pagination', async (t) => {
  t.mock.method(globalThis, 'fetch', async (_url, init) => {
    assert.equal(init.headers.authorization, 'Bearer secret');
    const request = JSON.parse(init.body);
    assert.equal(request.method, 'run/list');
    assert.deepEqual(request.params, { limit: 2, offset: 3 });
    return Response.json({ jsonrpc: '2.0', id: request.id, result: [] });
  });
  assert.deepEqual(await new OpenForgeClient('http://test', 'secret').listRuns(2, 3), []);
});

test('RPC rejects mismatched response IDs', async (t) => {
  t.mock.method(globalThis, 'fetch', async () => Response.json({ jsonrpc: '2.0', id: 99, result: [] }));
  await assert.rejects(new OpenForgeClient().listRuns(), /mismatched/);
});

test('stream parses split UTF-8, CRLF and heartbeat frames', async (t) => {
  const bytes = new TextEncoder().encode(': ping\r\n\r\nevent: audit\r\ndata: {"sequence":2,"text":"✓"}\r\n\r\nevent: audit\ndata: {"sequence":3}\n\n');
  t.mock.method(globalThis, 'fetch', async (url, init) => {
    assert.match(url, /after_sequence=1$/);
    assert.equal(init.headers.authorization, 'Bearer secret');
    return new Response(new ReadableStream({ start(controller) {
      for (const byte of bytes) controller.enqueue(Uint8Array.of(byte));
      controller.close();
    } }));
  });
  const events = [];
  for await (const event of new OpenForgeClient('http://test/', 'secret').streamEvents('run', 1)) events.push(event);
  assert.deepEqual(events, [{ sequence: 2, text: '✓' }, { sequence: 3 }]);
});

test('leaving stream cancels the response reader', async (t) => {
  let cancelled = false;
  t.mock.method(globalThis, 'fetch', async () => new Response(new ReadableStream({
    start(controller) { controller.enqueue(new TextEncoder().encode('event: audit\ndata: {"sequence":1}\n\n')); },
    cancel() { cancelled = true; },
  })));
  for await (const _ of new OpenForgeClient().streamEvents('run')) break;
  assert.equal(cancelled, true);
});

test('stream surfaces server errors', async (t) => {
  t.mock.method(globalThis, 'fetch', async () => new Response('event: error\ndata: storage unavailable\n\n'));
  await assert.rejects(async () => {
    for await (const _ of new OpenForgeClient().streamEvents('run')) { /* consume */ }
  }, /storage unavailable/);
});
