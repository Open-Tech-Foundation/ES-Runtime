// The shop's state: one durable worker per customer, per product and per order.
//
// This module is also what every shard imports, so it exports the classes and
// does nothing else at its top level. Each class names its storage explicitly:
// a minified build renames classes, and the name is where the state lives.

import { DurableWorker } from "runtime:workers";
import { price, product } from "../shared/catalog.js";

/** How long a cart holds stock after its last change. */
export const CART_TTL = 2 * 60_000;

/** How many times an order's webhook is tried before it is marked failed. */
export const DELIVERY_ATTEMPTS = 6;

/**
 * One product's stock. Every reservation and sale for the product goes through
 * this worker's mailbox, one at a time, so two customers can never both take
 * the last one — without a lock, a transaction or a second service.
 */
export class Inventory extends DurableWorker {
  static durableName = "Inventory";
  // A sale is recorded by order id, which is what makes `commit` safe to repeat
  // after a crash. It is a collection rather than a key because it only grows.
  static schema = { collections: { sales: { index: ["customer"] } } };

  #holds() {
    return this.state.get("holds") ?? new Map();
  }

  /** What can still be put in a cart. */
  available() {
    const held = [...this.#holds().values()].reduce((a, b) => a + b, 0);
    return (product(this.id)?.stock ?? 0) - (this.state.get("sold") ?? 0) - held;
  }

  /**
   * Holds `qty` for `customer`, replacing what it held before. Answers how many
   * it got, which is less than asked for when the stock has run out.
   */
  reserve(customer, qty) {
    const holds = this.#holds();
    const mine = holds.get(customer) ?? 0;
    const granted = Math.max(0, Math.min(qty, this.available() + mine));
    if (granted === 0) holds.delete(customer);
    else holds.set(customer, granted);
    this.state.set("holds", holds);
    return granted;
  }

  release(customer) {
    const holds = this.#holds();
    if (!holds.delete(customer)) return;
    this.state.set("holds", holds);
  }

  /** Turns `customer`'s hold into a sale for `orderId`. Repeating it is a no-op. */
  async commit(customer, orderId, qty) {
    const sales = this.state.collection("sales");
    if ((await sales.get(orderId)) !== undefined) return;
    await this.state.transaction(async () => {
      await sales.insert({ id: orderId, customer, qty, at: Date.now() });
      const holds = this.#holds();
      holds.delete(customer);
      this.state.set("holds", holds);
      this.state.set("sold", (this.state.get("sold") ?? 0) + qty);
    });
  }
}

/**
 * A customer: their cart, their order history, and a checkout that survives
 * the process dying halfway through it.
 */
export class Customer extends DurableWorker {
  static durableName = "Customer";
  static schema = { collections: { orders: { index: ["createdAt"] } } };

  // A checkout interrupted by a crash is finished on the next wake, before any
  // call is answered. Every step it repeats is idempotent.
  async start() {
    const pending = this.state.get("checkout");
    if (pending) await this.#finish(pending);
  }

  #cart() {
    return this.state.get("cart") ?? [];
  }

  view() {
    const alarm = this.state.alarm.get();
    return { ...price(this.#cart()), expiresAt: alarm ? alarm.getTime() : null };
  }

  /** Sets how many of `id` are in the cart. Answers the cart, and how many the
   * shop could actually hold for it. */
  async setQty(id, qty) {
    if (product(id) === undefined) throw new TypeError(`no product ${JSON.stringify(id)}`);
    const want = Math.max(0, Math.floor(qty));
    const granted = await Inventory.get(id).reserve(this.id, want);
    const cart = this.#cart().filter((line) => line.id !== id);
    if (granted > 0) cart.push({ id, qty: granted });
    this.state.set("cart", cart);
    if (cart.length > 0) await this.state.alarm.set(Date.now() + CART_TTL);
    else await this.state.alarm.delete();
    return { cart: this.view(), granted };
  }

  /** A cart left alone gives its stock back. */
  async alarm() {
    for (const line of this.#cart()) await Inventory.get(line.id).release(this.id);
    this.state.set("cart", []);
  }

  async checkout(webhook) {
    const { lines, total } = price(this.#cart());
    if (lines.length === 0) throw new RangeError("the cart is empty");
    const pending = { orderId: crypto.randomUUID(), lines, total, webhook };
    // The intent is written down first, and waited for: the calls below leave
    // this worker, and only the *result* of a call is gated on its writes.
    this.state.set("checkout", pending);
    await this.state.sync();
    return this.#finish(pending);
  }

  async #finish(pending) {
    for (const line of pending.lines) {
      await Inventory.get(line.id).commit(this.id, pending.orderId, line.qty);
    }
    const order = {
      id: pending.orderId,
      createdAt: Date.now(),
      lines: pending.lines,
      total: pending.total,
      delivery: "pending",
      attempts: 0,
    };
    const orders = this.state.collection("orders");
    if ((await orders.get(order.id)) === undefined) await orders.insert(order);
    await Delivery.get(order.id).schedule({ order, customer: this.id, webhook: pending.webhook });
    this.state.set("cart", []);
    await this.state.delete("checkout");
    await this.state.alarm.delete();
    return order;
  }

  /**
   * The newest orders, with where each one's delivery has got to. Delivery is
   * asked rather than told: a delivery that called back into its customer would
   * be a cycle, and two workers waiting on each other wait for ever.
   */
  async orders() {
    const orders = this.state.collection("orders");
    const newest = await orders.find().sort({ createdAt: "desc" }).limit(20).toArray();
    for (const order of newest) {
      if (order.delivery !== "pending") continue;
      const { status, attempts } = await Delivery.get(order.id).status();
      order.delivery = status;
      order.attempts = attempts;
      if (status !== "pending") await orders.update(order.id, { delivery: status, attempts });
    }
    return newest;
  }
}

/**
 * One order's fulfillment webhook, delivered at least once. The partner is
 * called from `alarm()`, so a failure is retried on a timer that survives a
 * restart, with the order id as the idempotency key for the repeats.
 */
export class Delivery extends DurableWorker {
  static durableName = "Delivery";

  async schedule(shipment) {
    if (this.state.has("shipment")) return;
    this.state.setMany({ shipment, status: "pending", attempts: 0 });
    await this.state.alarm.set(Date.now());
  }

  status() {
    return { status: this.state.get("status") ?? "pending", attempts: this.state.get("attempts") ?? 0 };
  }

  async alarm() {
    const shipment = this.state.get("shipment");
    if (!shipment) return;
    const attempts = (this.state.get("attempts") ?? 0) + 1;
    // Recorded before the call, and made durable: the call is the side effect
    // the gate does not cover.
    await this.state.set("attempts", attempts);
    let ok = false;
    try {
      const res = await fetch(shipment.webhook, {
        method: "POST",
        headers: { "content-type": "application/json", "idempotency-key": shipment.order.id },
        body: JSON.stringify({ order: shipment.order, customer: shipment.customer }),
      });
      ok = res.ok;
      await res.body?.cancel();
    } catch {
      ok = false;
    }
    if (ok) {
      this.state.set("status", "delivered");
    } else if (attempts >= DELIVERY_ATTEMPTS) {
      this.state.set("status", "failed");
    } else {
      // Our own backoff rather than a thrown error: the scheduler's retries end
      // in `onError`, and nothing there says which order gave up.
      await this.state.alarm.set(Date.now() + 500 * 2 ** (attempts - 1));
    }
  }
}
