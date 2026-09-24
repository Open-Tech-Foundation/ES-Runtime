// The storefront's view of the server. Every call carries the session cookie,
// which is what names the customer's durable worker.

async function call(method, path, body) {
  const res = await fetch(path, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? {} : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await res.json();
  if (!res.ok) throw new Error(data.error ?? res.statusText);
  return data;
}

export const api = {
  products: () => call("GET", "/api/products"),
  cart: () => call("GET", "/api/cart"),
  setQty: (id, qty) => call("POST", "/api/cart", { id, qty }),
  checkout: () => call("POST", "/api/checkout"),
  orders: () => call("GET", "/api/orders"),
};
