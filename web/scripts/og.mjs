// Render the link-preview card (route #/og) of a built site to og.png.
//   node scripts/og.mjs <dist dir> [--channel chrome]
// Serves the directory locally, waits for the chart, and screenshots the card.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { chromium } from "playwright-core";

const dir = process.argv[2] ?? "dist";
const channel = process.argv.includes("--channel") ? process.argv[process.argv.indexOf("--channel") + 1] : undefined;
const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml", ".woff2": "font/woff2", ".txt": "text/plain", ".jsonl": "text/plain" };
const server = createServer(async (req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^(\.\.[/\\])+/, "");
  const file = path === "/" || path === "\\" ? "index.html" : path;
  try {
    const body = await readFile(join(dir, file));
    res.writeHead(200, { "content-type": types[extname(file)] ?? "application/octet-stream" }).end(body);
  } catch {
    res.writeHead(404).end();
  }
}).listen(0);
const port = server.address().port;
const browser = await chromium.launch(channel ? { channel } : {});
const page = await browser.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 1, colorScheme: "dark" });
await page.goto(`http://localhost:${port}/#/og`);
await page.waitForSelector("#og-card svg path", { timeout: 30000 });
await page.waitForTimeout(800);
await page.locator("#og-card").screenshot({ path: join(dir, "og.png") });
await browser.close();
server.close();
console.log(`wrote ${join(dir, "og.png")}`);
