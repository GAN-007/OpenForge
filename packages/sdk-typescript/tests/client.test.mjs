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

test('runtime methods preserve policy paths, IDs and streamed payloads', async (t) => {
  const calls = [];
  t.mock.method(globalThis, 'fetch', async (_url, init) => {
    const request = JSON.parse(init.body);
    calls.push([request.method, request.params]);
    return Response.json({ jsonrpc: '2.0', id: request.id, result: {} });
  });
  const client = new OpenForgeClient();
  await client.leaseSecret({ secret_name: 'TEST', audience: 'worker', policy_path: '/policy' });
  await client.revokeSecret('lease');
  await client.acpRequest('process', 'echo', { text: 'ok' });
  await client.mcpCallTool('server', 'echo', { text: 'ok' }, '/policy');
  await client.mcpListTools('server', '/policy');
  await client.mcpListResources('server', '/policy');
  await client.mcpReadResource('server', 'test://value', '/policy');
  await client.mcpListPrompts('server', '/policy');
  await client.budgetReserve('run', .1);
  await client.budgetSettle('reservation', .05);
  await client.artifactStreamChunk('upload', 'YQ==');
  await client.artifactStreamCommit('upload', { source: 'test' });
  await client.validatePluginCapability('dev.test.plugin', 'browser:navigate');
  assert.deepEqual(calls, [
    ['secret/lease', { secret_name: 'TEST', audience: 'worker', policy_path: '/policy' }],
    ['secret/revoke', { lease_id: 'lease' }],
    ['acp/request', { process_id: 'process', method: 'echo', params: { text: 'ok' } }],
    ['mcp/call_tool', { server_name: 'server', tool_name: 'echo', arguments: { text: 'ok' }, policy_path: '/policy' }],
    ['mcp/list_tools', { server_name: 'server', policy_path: '/policy' }],
    ['mcp/list_resources', { server_name: 'server', policy_path: '/policy' }],
    ['mcp/read_resource', { server_name: 'server', uri: 'test://value', policy_path: '/policy' }],
    ['mcp/list_prompts', { server_name: 'server', policy_path: '/policy' }],
    ['budget/reserve', { run_id: 'run', estimated_usd: .1 }],
    ['budget/settle', { reservation_id: 'reservation', actual_usd: .05 }],
    ['artifact/stream/chunk', { upload_id: 'upload', base64: 'YQ==' }],
    ['artifact/stream/commit', { upload_id: 'upload', metadata: { source: 'test' } }],
    ['plugins/capability/validate', { plugin_id: 'dev.test.plugin', capability: 'browser:navigate' }],
  ]);
});
