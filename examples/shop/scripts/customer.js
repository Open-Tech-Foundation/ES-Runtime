// A customer, in a real browser: headless Chrome driven over the DevTools
// protocol, clicking through the shop and taking a screenshot at each step.
//
//     esdev scripts/customer.js [http://localhost:8080]
//
// The shop has to be running (`esdev start`). Screenshots land in
// `screenshots/`. Nothing here is part of the shop; it is how the demo is
// looked at the way a customer would see it.

import { makeTempDir, mkdir, remove, write } from "runtime:fs";
import { args } from "runtime:process";
import { Command } from "runtime:system";

const shop = args[0] ?? "http://localhost:8080";
const port = 9333;
// Beside the scripts directory, not inside it: a relative path here resolves
// against this file.
const out = new URL("../screenshots/", import.meta.url);

// A fresh profile per run, so the customer starts with no cookie. It goes in
// the ignored `.cache/`: Chrome writes to it constantly, and anywhere else in
// the project those writes would make `esdev start` rebuild and reload the page.
const cache = new URL("../.cache/", import.meta.url);
await mkdir(cache, { recursive: true });
const profile = await makeTempDir({ dir: cache.pathname, prefix: "customer-" });

const chrome = await new Command("google-chrome", {
  args: [
    "--headless=new",
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    "--window-size=1280,900",
    "--no-first-run",
    "about:blank",
  ],
  stdout: "null",
  stderr: "null",
}).spawn();

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function target() {
  for (let i = 0; i < 100; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
      const page = list.find((t) => t.type === "page");
      if (page) return page.webSocketDebuggerUrl;
    } catch {
      // Not listening yet.
    }
    await sleep(100);
  }
  throw new Error("Chrome did not start");
}

const socket = new WebSocket(await target());
await new Promise((resolve, reject) => {
  socket.onopen = resolve;
  socket.onerror = reject;
});

let seq = 0;
const waiting = new Map();
socket.onmessage = (event) => {
  const message = JSON.parse(String(event.data));
  const waiter = waiting.get(message.id);
  if (!waiter) return;
  waiting.delete(message.id);
  if (message.error) waiter.reject(new Error(message.error.message));
  else waiter.resolve(message.result);
};

function send(method, params = {}) {
  const id = ++seq;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => waiting.set(id, { resolve, reject }));
}

async function evaluate(expression) {
  const { result, exceptionDetails } = await send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (exceptionDetails) {
    const detail = exceptionDetails.exception?.description ?? exceptionDetails.text;
    throw new Error(`${detail}\n  in: ${expression}`);
  }
  return result.value;
}

async function until(what, expression, timeout = 10_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await evaluate(expression)) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${what}`);
}

let shot = 0;
async function screenshot(name) {
  const { data } = await send("Page.captureScreenshot", { format: "png" });
  const bytes = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
  const path = new URL(`${String(++shot).padStart(2, "0")}-${name}.png`, out);
  await write(path, bytes);
  console.log(`  screenshots/${path.pathname.split("/").pop()}`);
}

// The page's own words, which is what a customer reads.
const text = (selector) =>
  `(document.querySelector(${JSON.stringify(selector)})?.textContent ?? "")`;
const card = (name) => `[...document.querySelectorAll("article")]
    .find((a) => a.querySelector("h2").textContent.includes(${JSON.stringify(name)}))`;
// As a person would: wait until the button can be pressed, then press it.
async function addToCart(name) {
  await until(`${name} to be addable`, `!${card(name)}.querySelector("button").disabled`).catch(() => {});
  await evaluate(`${card(name)}.querySelector("button").click()`);
}
const cartCount = () => evaluate(text("[data-testid=cart-count]"));

async function step(title, run) {
  console.log(`• ${title}`);
  await run();
}

try {
  await mkdir(out, { recursive: true });
  await send("Page.enable");
  await send("Page.navigate", { url: shop });

  await step("opens the shop", async () => {
    await until("the products", `document.querySelectorAll("article").length > 0`);
    await screenshot("storefront");
  });

  // What is on the shelf decides what this customer buys, as it would a person:
  // the two best-stocked products, and later the scarcest one.
  const shelf = await evaluate(`fetch("/api/products").then((r) => r.json())`);
  const stocked = shelf.filter((p) => p.available > 0).sort((a, b) => b.available - a.available);
  const [first, second] = stocked;
  if (!first || !second) throw new Error("the shop is sold out — delete .dev/dist/.durable to restock");

  await step(`adds two of ${first.name} and one ${second.name}`, async () => {
    await addToCart(first.name);
    await until("one item", `${text("[data-testid=cart-count]")}.includes("1")`);
    await addToCart(first.name);
    await until("two items", `${text("[data-testid=cart-count]")}.includes("2")`);
    await addToCart(second.name);
    await until("three items", `${text("[data-testid=cart-count]")}.includes("3")`);
    console.log(`  cart: ${await cartCount()}`);
    await screenshot("cart");
  });

  await step("reloads — the cart is still there", async () => {
    await send("Page.reload");
    await until("the cart back", `${text("[data-testid=cart-count]")}.includes("3")`);
    await screenshot("after-reload");
  });

  await step("checks out", async () => {
    await evaluate(`document.querySelector("button.primary").click()`);
    await until("the order", `document.querySelectorAll(".orders li").length > 0`);
    await screenshot("ordered");
  });

  await step("waits for the partner to confirm delivery", async () => {
    await until("delivery", `!!document.querySelector(".orders .status.delivered")`, 30_000);
    await screenshot("delivered");
  });

  await step("tries to buy more than the shop has", async () => {
    const now = await evaluate(`fetch("/api/products").then((r) => r.json())`);
    const scarce = now.filter((p) => p.available > 0).sort((a, b) => a.available - b.available)[0];
    if (!scarce) return;
    console.log(`  ${scarce.name}: ${scarce.available} left`);
    for (let i = 0; i <= scarce.available; i++) {
      if (await evaluate(`${card(scarce.name)}.querySelector("button").disabled && !document.querySelector("button.primary")?.disabled`)) break;
      await addToCart(scarce.name);
      await sleep(400);
    }
    // The last press takes the cart past what is left: the shop keeps what it
    // could hold and says so, or the product shows as sold out.
    await until("sold out", `${card(scarce.name)}.textContent.includes("Sold out")`);
    console.log(`  notice: ${await evaluate(text(".notice")) || "(none)"}`);
    await screenshot("stock-limit");
  });
} catch (e) {
  await screenshot("failure").catch(() => {});
  throw e;
} finally {
  socket.close();
  await chrome.kill();
  await remove(profile, { recursive: true }).catch(() => {});
}
