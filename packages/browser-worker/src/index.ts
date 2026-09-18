import { createInterface } from "node:readline";
import { chromium, type Browser, type Page } from "playwright";

type Request = {
  id: number;
  method: string;
  params?: Record<string, unknown>;
};

let browser: Browser | undefined;
let page: Page | undefined;

async function ensure(): Promise<Page> {
  if (!browser) browser = await chromium.launch({ headless: true });
  if (!page) page = await browser.newPage();
  return page;
}

function safeUrl(raw: string): string {
  const url = new URL(raw);
  if (!["http:", "https:"].includes(url.protocol)) {
    throw new Error("only http/https navigation is permitted");
  }
  return url.toString();
}

async function dispatch(request: Request): Promise<unknown> {
  const activePage = await ensure();
  const params = request.params ?? {};
  const timeout = Number(params.timeout_ms ?? 10_000);

  switch (request.method) {
    case "navigate":
      await activePage.goto(safeUrl(String(params.url)), {
        waitUntil: "domcontentloaded",
        timeout: Number(params.timeout_ms ?? 30_000),
      });
      return { url: activePage.url(), title: await activePage.title() };
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
