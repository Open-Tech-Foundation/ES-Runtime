// The catalog, and the arithmetic both halves of the shop agree on. Prices are
// integer cents, so no total is ever a floating-point guess.

export const PRODUCTS = [
  { id: "mug", name: "Enamel mug", emoji: "☕", price: 1400, stock: 25 },
  { id: "tee", name: "Runtime tee", emoji: "👕", price: 2400, stock: 40 },
  { id: "cap", name: "Snapback cap", emoji: "🧢", price: 1900, stock: 15 },
  { id: "tote", name: "Canvas tote", emoji: "👜", price: 1600, stock: 30 },
  { id: "pin", name: "Enamel pin", emoji: "📍", price: 500, stock: 100 },
  { id: "hoodie", name: "Heavy hoodie", emoji: "🧥", price: 5200, stock: 5 },
];

export const product = (id) => PRODUCTS.find((p) => p.id === id);

/** `[{ id, qty }]` priced from the catalog. A line for a product the catalog no
 * longer has is dropped rather than priced at zero. */
export function price(lines) {
  const priced = [];
  for (const line of lines) {
    const p = product(line.id);
    if (p === undefined || line.qty <= 0) continue;
    priced.push({ ...line, name: p.name, emoji: p.emoji, price: p.price, subtotal: p.price * line.qty });
  }
  return { lines: priced, total: priced.reduce((sum, l) => sum + l.subtotal, 0) };
}

export const money = (cents) => `$${(cents / 100).toFixed(2)}`;
