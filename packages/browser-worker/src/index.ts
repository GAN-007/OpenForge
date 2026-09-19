import { lookup } from "node:dns/promises";
import { isIP } from "node:net";
import { createInterface } from "node:readline";
import {
  chromium,
  type Browser,
  type BrowserContext,
  type Page,
  type Request as PlaywrightRequest,
  type Response,
} from "playwright";

type Request = {
  id: number;
  method: string;
  params?: Record<string, unknown>;
};

type NetworkEntry = {
  kind: "request" | "response";
  method?: string;
  url: string;
  status?: number;
  resourceType?: string;
  timestamp: number;
};

let browser: Browser | undefined;
let context: BrowserContext | undefined;
const pages = new Map<string, Page>();
let activePageId: string | undefined;
let allowLocalRequests = false;
let networkEntries: NetworkEntry[] = [];
let consoleEntries: string[] = [];

async function ensureBrowser(): Promise<Browser> {
  if (!browser) {
    browser = await chromium.launch({ headless: true });
  }
  return browser;
}

async function ensureContext(): Promise<BrowserContext> {
  if (!context) {
    const activeBrowser = await ensureBrowser();
    context = await activeBrowser.newContext();
    attachContextListeners(context);
    await createPage(context);
  }
  return context;
}

async function createPage(target: BrowserContext): Promise<{ id: string; page: Page }> {
  const page = await target.newPage();
  const id = crypto.randomUUID();
  pages.set(id, page);
  activePageId = id;
  await page.route("**/*", async (route) => {
    try {
      await validateUrl(route.request().url(), allowLocalRequests);
      await route.continue();
    } catch {
      await route.abort("blockedbyclient");
    }
  });
  page.on("console", (message) => {
    consoleEntries.push(message.type() + ": " + message.text());
    if (consoleEntries.length > 5000) consoleEntries = consoleEntries.slice(-5000);
  });
  return { id, page };
}

function activePage(): Page {
  if (!activePageId) throw new Error("no active browser page");
  const page = pages.get(activePageId);
  if (!page) throw new Error("active browser page is unavailable");
  return page;
}

function attachContextListeners(target: BrowserContext) {
  target.on("request", (request: PlaywrightRequest) => {
    networkEntries.push({
      kind: "request",
      method: request.method(),
      url: request.url(),
      resourceType: request.resourceType(),
      timestamp: Date.now(),
    });
    trimNetwork();
  });
  target.on("response", (response: Response) => {
    networkEntries.push({
      kind: "response",
      url: response.url(),
      status: response.status(),
      timestamp: Date.now(),
    });
    trimNetwork();
  });
}

function trimNetwork() {
  if (networkEntries.length > 10_000) {
    networkEntries = networkEntries.slice(-10_000);
  }
}

function isLocalHost(hostname: string): boolean {
  const normalized = hostname.toLowerCase().replace(/^\[|\]$/g, "");
  return (
    normalized === "localhost" ||
    normalized.endsWith(".localhost") ||
    normalized === "127.0.0.1" ||
    normalized === "::1"
  );
}

function blockedIpv4(address: string): boolean {
  const parts = address.split(".").map(Number);
  if (parts.length !== 4 || parts.some((part) => !Number.isInteger(part))) {
    return true;
  }
  const a = parts[0]!;
  const b = parts[1]!;
  return (
    a === 0 ||
    a === 10 ||
    a === 127 ||
    (a === 169 && b === 254) ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168) ||
    (a === 100 && b >= 64 && b <= 127) ||
    a >= 224
  );
}

function blockedIpv6(address: string): boolean {
  const value = address.toLowerCase();
  if (value === "::" || value === "::1") return true;
  if (value.startsWith("fc") || value.startsWith("fd")) return true;
  if (/^fe[89ab]/.test(value)) return true;
  if (value.startsWith("ff")) return true;
  if (value.startsWith("::ffff:")) {
    const mapped = value.slice("::ffff:".length);
    return isIP(mapped) === 4 ? blockedIpv4(mapped) : true;
  }
  return false;
}

function blockedAddress(address: string): boolean {
  const family = isIP(address);
  if (family === 4) return blockedIpv4(address);
  if (family === 6) return blockedIpv6(address);
  return true;
}

async function validateUrl(raw: string, allowLocal: boolean): Promise<string> {
  const url = new URL(raw);
  if (!["http:", "https:"].includes(url.protocol)) {
    throw new Error("only http/https navigation is permitted");
  }
  if (url.username || url.password) {
    throw new Error("URL userinfo is not permitted");
  }
  if (isLocalHost(url.hostname)) {
    if (!allowLocal) {
      throw new Error("localhost access requires an explicit localhost navigation");
    }
    return url.toString();
  }

  const records = await lookup(url.hostname, { all: true, verbatim: true });
  if (!records.length) throw new Error("hostname did not resolve");
  if (records.some((record) => blockedAddress(record.address))) {
    throw new Error("navigation resolved to a blocked private or special-use address");
  }
  return url.toString();
}

async function newContext(params: Record<string, unknown>) {
  await context?.close();
  pages.clear();
  activePageId = undefined;
  networkEntries = [];
  consoleEntries = [];
  const activeBrowser = await ensureBrowser();
  const viewport =
    params.viewport_width && params.viewport_height
      ? {
          width: Number(params.viewport_width),
          height: Number(params.viewport_height),
        }
      : undefined;
  const recordVideo =
    typeof params.video_dir === "string" && params.video_dir
      ? { dir: params.video_dir }
      : undefined;
  const recordHar =
    typeof params.har_path === "string" && params.har_path
      ? { path: params.har_path, mode: "full" as const, content: "embed" as const }
      : undefined;
  context = await activeBrowser.newContext({
    viewport,
    locale: typeof params.locale === "string" ? params.locale : undefined,
    userAgent:
      typeof params.user_agent === "string" ? params.user_agent : undefined,
    recordVideo,
    recordHar,
  });
  attachContextListeners(context);
  const created = await createPage(context);
  return { page_id: created.id };
}

async function dispatch(request: Request): Promise<unknown> {
  const params = request.params ?? {};
  const timeout = Number(params.timeout_ms ?? 10_000);

  switch (request.method) {
    case "context/new":
      return newContext(params);
    case "page/new": {
      const target = await ensureContext();
      const created = await createPage(target);
      return { page_id: created.id };
    }
    case "page/list":
      return {
        active_page_id: activePageId,
        pages: await Promise.all(
          [...pages.entries()].map(async ([id, page]) => ({
            id,
            url: page.url(),
            title: await page.title(),
          })),
        ),
      };
    case "page/switch": {
      const id = String(params.page_id);
      if (!pages.has(id)) throw new Error("unknown browser page " + id);
      activePageId = id;
      return { page_id: id };
    }
    case "page/close": {
      const id = String(params.page_id);
      const page = pages.get(id);
      if (!page) return { closed: false };
      await page.close();
      pages.delete(id);
      if (activePageId === id) activePageId = pages.keys().next().value;
      return { closed: true };
    }
  }

  await ensureContext();
  const page = activePage();

  switch (request.method) {
    case "navigate": {
      const raw = String(params.url);
      const parsed = new URL(raw);
      allowLocalRequests = isLocalHost(parsed.hostname);
      const target = await validateUrl(raw, allowLocalRequests);
      await page.goto(target, {
        waitUntil: "domcontentloaded",
        timeout: Number(params.timeout_ms ?? 30_000),
      });
      return { url: page.url(), title: await page.title(), page_id: activePageId };
    }
    case "click":
      await page.locator(String(params.selector)).click({ timeout });
      return { url: page.url(), page_id: activePageId };
    case "fill":
      await page
        .locator(String(params.selector))
        .fill(String(params.value ?? ""), { timeout });
      return { ok: true };
    case "press":
      await page.locator(String(params.selector)).press(String(params.key), { timeout });
      return { ok: true };
    case "text":
      return {
        text: await page
          .locator(String(params.selector ?? "body"))
          .innerText({ timeout }),
      };
    case "html":
      return {
        html: await page
          .locator(String(params.selector ?? "body"))
          .innerHTML({ timeout }),
      };
    case "dom/snapshot":
      return {
        url: page.url(),
        title: await page.title(),
        html: await page.content(),
      };
    case "accessibility/snapshot":
      return {
        snapshot: await page.locator(String(params.selector ?? "body")).ariaSnapshot(),
      };
    case "screenshot": {
      const data = await page.screenshot({
        fullPage: Boolean(params.full_page ?? true),
      });
      return { base64: data.toString("base64"), url: page.url(), page_id: activePageId };
    }
    case "network/clear":
      networkEntries = [];
      return { ok: true };
    case "network/entries":
      return {
        entries: networkEntries.slice(-Math.min(10_000, Number(params.limit ?? 1000))),
      };
    case "console/clear":
      consoleEntries = [];
      return { ok: true };
    case "console":
      return {
        entries: consoleEntries.slice(-Math.min(5000, Number(params.limit ?? 1000))),
      };
    case "trace/start":
      await context!.tracing.start({
        screenshots: true,
        snapshots: true,
        sources: true,
      });
      return { ok: true };
    case "trace/stop": {
      const path = String(params.path);
      if (!path) throw new Error("trace/stop requires path");
      await context!.tracing.stop({ path });
      return { path };
    }
    case "storage/state":
      return context!.storageState();
    case "storage/save": {
      const path = String(params.path);
      if (!path) throw new Error("storage/save requires path");
      return context!.storageState({ path });
    }
    case "close":
      await context?.close();
      await browser?.close();
      browser = undefined;
      context = undefined;
      pages.clear();
      activePageId = undefined;
      allowLocalRequests = false;
      networkEntries = [];
      consoleEntries = [];
      return { ok: true };
    default:
      throw new Error("unknown browser method " + request.method);
  }
}

const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });

for await (const line of rl) {
  if (!line.trim()) continue;
  let id: number | null = null;
  try {
    const request = JSON.parse(line) as Request;
    id = request.id;
    const result = await dispatch(request);
    process.stdout.write(
      JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n",
    );
  } catch (error) {
    process.stdout.write(
      JSON.stringify({
        jsonrpc: "2.0",
        id,
        error: {
          code: -32000,
          message: error instanceof Error ? error.message : String(error),
        },
      }) + "\n",
    );
  }
}

process.on("SIGTERM", () => {
  void context?.close().finally(() =>
    browser?.close().finally(() => process.exit(0)),
  );
});
