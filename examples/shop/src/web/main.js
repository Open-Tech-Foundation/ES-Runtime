import { cx, define, html, mount, onReady, update } from "@opentf/micro-ui";
import { money } from "../shared/catalog.js";
import { api } from "./api.js";

define("x-shop", (el) => {
  let products = [];
  let cart = { lines: [], total: 0, expiresAt: null };
  let orders = [];
  let notice = "";
  let busy = false;

  const say = (text) => {
    notice = text;
    update(el);
  };

  async function refresh() {
    try {
      [products, cart, orders] = await Promise.all([api.products(), api.cart(), api.orders()]);
    } catch (e) {
      notice = e.message;
    }
    update(el);
  }

  const inCart = (id) => cart.lines.find((l) => l.id === id)?.qty ?? 0;

  async function setQty(id, qty) {
    busy = true;
    update(el);
    try {
      const res = await api.setQty(id, qty);
      cart = res.cart;
      if (res.granted < qty) say(`Only ${res.granted} left — your cart holds what we had.`);
      else notice = "";
      products = await api.products();
    } catch (e) {
      say(e.message);
    }
    busy = false;
    update(el);
  }

  async function checkout() {
    busy = true;
    update(el);
    try {
      const order = await api.checkout();
      say(`Order ${order.id.slice(0, 8)} placed — ${money(order.total)}.`);
      await refresh();
    } catch (e) {
      say(e.message);
    }
    busy = false;
    update(el);
  }

  // Stock is pushed: the shelf tells every open page when a number changes.
  // Reconnects on its own, and a heartbeat keeps idle proxies from closing it.
  let socket = null;
  let beat = null;
  function live() {
    socket = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/live`);
    socket.onmessage = (e) => {
      if (e.data === "pong") return;
      const { id, available } = JSON.parse(e.data);
      products = products.map((p) => (p.id === id ? { ...p, available } : p));
      update(el);
    };
    socket.onclose = () => setTimeout(live, 1000);
  }

  onReady(() => {
    void refresh();
    live();
    beat = setInterval(() => socket?.readyState === 1 && socket.send("ping"), 25_000);
    // Deliveries finish on their own, and a held cart counts down.
    const poll = setInterval(async () => {
      try {
        orders = await api.orders();
        if (cart.expiresAt !== null && cart.expiresAt < Date.now()) cart = await api.cart();
      } catch {
        // The next tick tries again.
      }
      update(el);
    }, 2000);
    return () => {
      clearInterval(poll);
      clearInterval(beat);
      if (socket) {
        socket.onclose = null;
        socket.close();
      }
    };
  });

  const count = () => cart.lines.reduce((n, l) => n + l.qty, 0);

  const held = () => {
    if (cart.expiresAt === null) return "";
    const s = Math.max(0, Math.round((cart.expiresAt - Date.now()) / 1000));
    return `Held for ${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  };

  return () => html`
    <header class="bar">
      <h1>Durable Shop</h1>
      <span class="badge" data-testid="cart-count">🛒 ${count()}</span>
    </header>
    ${notice ? html`<p class="notice" role="status">${notice}</p>` : ""}
    <div class="layout">
      <section class="grid" aria-label="Products">
        ${products.map(
          (p) => html`
            <article key=${p.id} class=${cx("card", p.available === 0 && "sold-out")}>
              <div class="emoji">${p.emoji}</div>
              <h2>${p.name}</h2>
              <p class="price">${money(p.price)}</p>
              <p class="stock">${p.available === 0 ? "Sold out" : `${p.available} in stock`}</p>
              <button
                disabled=${busy || p.available === 0}
                onclick=${() => setQty(p.id, inCart(p.id) + 1)}
              >
                Add to cart
              </button>
            </article>
          `,
        )}
      </section>
      <aside class="panel">
        <h2>Cart</h2>
        ${cart.lines.length === 0
          ? html`<p class="muted">Your cart is empty.</p>`
          : html`
              <ul class="lines">
                ${cart.lines.map(
                  (l) => html`
                    <li key=${l.id}>
                      <span>${l.emoji} ${l.name}</span>
                      <span class="qty">
                        <button disabled=${busy} onclick=${() => setQty(l.id, l.qty - 1)} aria-label="Remove one">−</button>
                        ${l.qty}
                        <button disabled=${busy} onclick=${() => setQty(l.id, l.qty + 1)} aria-label="Add one">+</button>
                      </span>
                      <span>${money(l.subtotal)}</span>
                    </li>
                  `,
                )}
              </ul>
              <p class="total">Total <strong>${money(cart.total)}</strong></p>
              <p class="muted">${held()}</p>
              <button class="primary" disabled=${busy} onclick=${checkout}>Checkout</button>
            `}
        <h2>Orders</h2>
        ${orders.length === 0
          ? html`<p class="muted">No orders yet.</p>`
          : html`
              <ul class="orders">
                ${orders.map(
                  (o) => html`
                    <li key=${o.id}>
                      <span>#${o.id.slice(0, 8)}</span>
                      <span>${money(o.total)}</span>
                      <span class=${cx("status", o.delivery)}>
                        ${o.delivery === "pending" && o.attempts > 0
                          ? `retrying (${o.attempts})`
                          : o.delivery}
                      </span>
                    </li>
                  `,
                )}
              </ul>
            `}
      </aside>
    </div>
  `;
});

const root = document.getElementById("app");
if (root) mount(root, "x-shop");
