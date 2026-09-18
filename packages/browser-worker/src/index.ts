import { lookup } from "node:dns/promises";
import { isIP } from "node:net";
import { createInterface } from "node:readline";
import { chromium, type Browser, type Page } from "playwright";

type Request = {
  id: number;
  method: string;
  params?: Record<string, unknown>;
};

let browser: Browser | undefined;
let page: Page | undefined;
let allowLocalRequests = false;

async function ensure(): Promise<Page> {
  if (!browser) {
    browser = await chromium.launch({ headless: true });
  }
  if (!page) {
    page = await browser.newPage();
    await page.route("**/*", async (route) => {
      try {
        await validateUrl(route.request().url(), allowLocalRequests);
        await route.continue();
      } catch {
        await route.abort("blockedbyclient");
      }
    });
  }
  return page;
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

async function validateUrl(
  raw: string,
  allowLocal: boolean,
): Promise<string> {
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

  const records = await lookup(url.hostname, {
    all: true,
    verbatim: true,
  });
  if (!records.length) {
    throw new Error("hostname did not resolve");
  }
  if (records.some((record) => blockedAddress(record.address))) {
    throw new Error("navigation resolved to a blocked private or special-use address");
  }
  return url.toString();
}

async function dispatch(request: Request): Promise<unknown> {
  const activePage = await ensure();
  const params = request.params ?? {};
  const timeout = Number(params.timeout_ms ?? 10_000);

  switch (request.method) {
    case "navigate": {
      const raw = String(params.url);
      const parsed = new URL(raw);
      allowLocalRequests = isLocalHost(parsed.hostname);
      const target = await validateUrl(raw, allowLocalRequests);
      await activePage.goto(target, {
        waitUntil: "domcontentloaded",
        timeout: Number(params.timeout_ms ?? 30_000),
      });
      return { url: activePage.url(), title: await activePage.title() };
    }
    case "click":
      await activePage.locator(String(params.selector)).click({ timeout });
      return { url: activePage.url() };
    case "fill":
      await activePage
        .locator(String(params.selector))
        .fill(String(params.value ?? ""), { timeout });
      return { ok: true };
    case "text":
      return {
        text: await activePage
          .locator(String(params.selector ?? "body"))
          .innerText({ timeout }),
      };
    case "html":
      return {
        html: await activePage
          .locator(String(params.selector ?? "body"))
          .innerHTML({ timeout }),
      };
    case "screenshot": {
      const data = await activePage.screenshot({
        fullPage: Boolean(params.full_page ?? true),
      });
      return { base64: data.toString("base64"), url: activePage.url() };
    }
    case "console": {
      const entries: string[] = [];
      const listener = (message: { type(): string; text(): string }) =>
        entries.push(message.type() + ": " + message.text());
      activePage.on("console", listener);
      await activePage.waitForTimeout(Number(params.duration_ms ?? 1000));
      activePage.off("console", listener);
      return { entries };
    }
    case "close":
      await browser?.close();
      browser = undefined;
      page = undefined;
      allowLocalRequests = false;
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
  void browser?.close().finally(() => process.exit(0));
});
