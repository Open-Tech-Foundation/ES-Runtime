# Durable Shop

A small shop backend on ES Runtime's [durable workers](https://esrun.opentechf.org/api/workers),
with a [Micro-UI](https://micro-ui.opentechf.org/) storefront. Nothing runs
beside it: no database, cache or queue.

| Worker | One per | Holds |
| --- | --- | --- |
| `Customer` | session | the cart, a checkout in progress, the order history (a collection) |
| `Inventory` | product | reservations and sales; every hold goes through its mailbox, so the last item is never sold twice |
| `Delivery` | order | the fulfillment webhook, retried from `alarm()` until the partner accepts it |

A cart left alone for two minutes gives its stock back, from an alarm. The
worker code runs on shards; the state stays in the server process.

```sh
npm install
npm run dev              # esdev start — http://localhost:8080
npm test
esdev scripts/customer.js   # a customer in headless Chrome, with screenshots
```

`FAIL_RATE` (default `0.3`) is how often the fake fulfillment partner refuses a
webhook, so the retries are visible. `SHARDS` (default `2`) sets the pool.

State lives in `.durable/` beside the running server — `.dev/dist/.durable`
under `esdev start`. Delete it to restock.
