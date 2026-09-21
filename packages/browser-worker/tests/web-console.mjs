// Browser interaction smoke test with a deterministic control-plane transport.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const root = new URL('../../web-console/dist/', import.meta.url);
const server = createServer(async (request, response) => {
  try {
    const path = new URL(request.url, 'http://localhost').pathname;
    const file = new URL('.' + (path === '/' ? '/index.html' : path), root);
    if (!file.href.startsWith(root.href)) throw new Error('invalid path');
    const content = await readFile(fileURLToPath(file));
    const type = path.endsWith('.js') ? 'application/javascript' : path.endsWith('.css') ? 'text/css' : 'text/html';
    response.writeHead(200, { 'content-type': type });
    response.end(content);
  } catch { response.writeHead(404); response.end(); }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  let run = null;
  let tasks = [];
  let gateway = { connected: false, base_url: 'https://model.sevi.io/cursor', model: 'auto-select', credential_storage: 'user_config_file', credential_persisted: false, pricing: 'gateway_reported_or_unpriced' };
  const headers = { 'access-control-allow-origin': '*', 'access-control-allow-headers': '*', 'access-control-allow-methods': 'GET,POST,OPTIONS' };
  await page.route('http://127.0.0.1:8765/**', async (route) => {
    if (route.request().method() === 'OPTIONS') return route.fulfill({ status: 204, headers });
    if (route.request().url().endsWith('/health')) return route.fulfill({ json: { status: 'ok' }, headers });
    if (route.request().url().includes('/events/stream')) {
      const event = { sequence: 1, event_id: 'event-1', run_id: run.id, timestamp: new Date().toISOString(), actor: { kind: 'user', id: 'smoke' }, event_type: 'run.created', payload: {} };
      return route.fulfill({ contentType: 'text/event-stream', body: `event: audit\nid: 1\ndata: ${JSON.stringify(event)}\n\n`, headers });
    }
    const request = route.request().postDataJSON();
    let result;
    switch (request.method) {
      case 'gateway/status': result = gateway; break;
      case 'gateway/connect':
        assert.equal(request.params.api_key, 'fake-gateway-key');
        gateway = { ...gateway, connected: true, credential_persisted: true };
        result = gateway; break;
      case 'gateway/disconnect': gateway = { ...gateway, connected: false, credential_persisted: false }; result = gateway; break;
      case 'event/verify': result = { valid: true, verified_events: 1, violations: [] }; break;
      case 'telemetry/snapshot': result = { counters: {}, gauges: {}, histograms: {}, recent_events: [] }; break;
      case 'run/list': result = run ? [run] : []; break;
      case 'run/create':
        assert.equal(request.params.autonomy, 'suggest');
        assert.equal(request.params.repo, '/test/repository');
        run = { id: '00000000-0000-4000-8000-000000000001', objective: request.params.objective, status: 'planning', autonomy: 'suggest', base_sha: 'abc', budget: { hard_limit: request.params.budget_usd } };
        result = run; break;
      case 'run/get': result = run; break;
      case 'task/list': result = tasks; break;
      case 'budget/get': result = { spent_usd: 0 }; break;
      case 'run/plan':
        tasks = [{ id: 'task-1', title: 'Check repository', role: 'reviewer', status: 'pending', dependencies: [], requirements: { capabilities: [], resources: { cpu_cores: 1, memory_mb: 128 } } }];
        result = tasks; break;
      default: throw new Error(`Unexpected method: ${request.method}`);
    }
    await route.fulfill({ json: { jsonrpc: '2.0', id: request.id, result }, headers });
  });
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  const keyInput = page.getByLabel('Sevi API key', { exact: true });
  assert.equal(await keyInput.getAttribute('type'), 'password');
  await keyInput.fill('fake-gateway-key');
  await page.getByRole('button', { name: 'Test & connect', exact: true }).click();
  await page.getByText('Connected · key remembered', { exact: true }).waitFor();
  assert.equal(await keyInput.inputValue(), '');
  assert.equal(await page.evaluate(() => JSON.stringify([Object.entries(localStorage), Object.entries(sessionStorage)]).includes('fake-gateway-key')), false);
  await page.reload();
  await page.getByText('Connected · key remembered', { exact: true }).waitFor();
  assert.equal(await page.getByLabel('Sevi API key', { exact: true }).inputValue(), '');
  await page.getByRole('button', { name: 'Disconnect & forget key', exact: true }).click();
  await page.getByText('Not connected', { exact: true }).waitFor();
  await page.getByLabel('Repository path', { exact: true }).fill('/test/repository');
  await page.getByLabel('Objective', { exact: true }).fill('Audit smoke objective');
  await page.getByRole('button', { name: 'Create run', exact: true }).click();
  await page.getByRole('heading', { name: 'Audit smoke objective', exact: true }).waitFor();
  await page.getByText('run.created', { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Plan selected run', exact: true }).click();
  await page.locator('article.task').filter({ hasText: 'Check repository' }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Plan selected run', exact: true }).isDisabled(), true);
  assert.deepEqual(errors, []);
  console.log('Web smoke passed: remembered Sevi connect/disconnect, create run, select run, audit stream, plan tasks; no browser exceptions.');
} finally {
  await browser?.close();
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
}
