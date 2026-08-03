---
title: "Internals: networking"
description: What happens to a connection from accept to close — every limit, every default, and what each one costs.
---

# Internals: networking

What actually happens to a TCP connection between `accept` and `close`, which
limit applies at each stage, and why each default is the number it is.

This page is for people sizing a deployment, reading a packet capture, or
deciding what to put in front of the runtime. It explains behaviour rather than
listing signatures — for those, see the [`runtime:http`](/api/http) and
[`runtime:net`](/api/net) references. Where a decision had a real alternative,
the reasoning is recorded in `docs/DECISIONS.md` (D42–D45).

## The life of a connection

Every stage below can be a place a connection stops making progress, so every
stage has a bound.

```
listener.accept()
  │  ← blocked here while at maxConnections (permit)
  │  ← errors retried with 5ms→1s backoff, never fatal
  ▼
TLS handshake                          timeouts.handshake (10s)
  ▼
first byte / version detection         timeouts.handshake (10s)
  │    HTTP/2 preface? → h2c    else → HTTP/1.1
  ▼
request head                           timeouts.headerRead (30s)
  ▼
handler ── request body ── response    unbounded, by design
  ▼
idle                    h1: timeouts.headerRead · h2: timeouts.h2KeepAlive
  ▼
close
```

### Accept

The accept loop retries every error rather than ending. `ECONNABORTED` (a client
that hung up between the SYN and the accept), `EMFILE`/`ENFILE` (a momentarily
full descriptor table) and `EINTR` are ordinary on a busy public port and say
nothing about the listening socket. The wait between attempts doubles from 5ms
to a ceiling of 1s and resets on the next accepted connection, so one transient
failure costs 5ms while a persistent one settles at a wakeup per second instead
of spinning a core. Each retry logs at `warn` on the `runtime::http` target.

A loop that exited here would leave the port bound and the server dead — nothing
served, and nothing else able to take the address.

### TLS handshake and first byte

Both are bounded by `timeouts.handshake`, and a TLS connection passes through
both, so it can take up to twice that value before it counts as established.

They are separate mechanisms. The handshake is wrapped in a timeout because
rustls will otherwise wait for the peer's next flight indefinitely. The
first-byte wait cannot be: version detection reads up to 24 bytes — the length
of the HTTP/2 connection preface — *before* either kind of hyper connection
exists, so no timer hyper owns is running yet. A wrapper around the stream
applies the deadline until one byte arrives and goes inert afterwards, because a
long-lived connection must not be interrupted by it.

### Request head, and the idle connection

`timeouts.headerRead` bounds how long a request head may take to arrive. On
HTTP/1.1 it is **also the idle keep-alive limit** — not a second timer, the same
one. hyper arms it whenever it waits for a request head, and waiting for the
*next* request on a kept-alive connection is exactly that. At the default, an
idle connection is closed after 30s and a client that wants another request
opens a new one.

HTTP/2 does not work this way. Its connections are long-lived by design, so
there is no idle limit; instead an idle connection is probed with a PING every
`timeouts.h2KeepAlive`, and dropped if the ACK does not arrive within the same
interval. Without probing, a peer that vanishes *without a FIN* — a NAT that
dropped the mapping, a killed VM, an unplugged cable — keeps its connection and
its share of the stream budget until the OS TCP keepalive notices, which is two
hours by default on Linux.

### What is deliberately not bounded

A request in flight, a body still arriving, and a response still streaming have
no deadline, however long they take. A live feed, a slow query, and a large
download are all indistinguishable from a stalled connection if you only look at
elapsed time, so elapsed time is not what these timeouts look at.

Also unbounded today: a slow request *body* (a one-byte-per-minute upload), and
total request duration. Both are known gaps rather than decisions that came out
the other way — a total cap would break SSE and long-polling, and there is no
per-route override to escape it.

## Every limit, in one place

| Limit | Default | Scope | Set by |
| --- | --- | --- | --- |
| `timeouts.handshake` | 10s | per connection | `serve({ timeouts })` |
| `timeouts.headerRead` | 30s | per connection | `serve({ timeouts })` |
| `timeouts.h2KeepAlive` | 20s | per connection | `serve({ timeouts })` |
| `maxConnections` | unlimited | per server | `serve({ maxConnections })` |
| HTTP/1.1 header fields | 100 | per request | fixed |
| HTTP/1.1 read buffer | ~408KB | per connection | fixed |
| HTTP/2 header list | 16KB | per request | fixed, advertised in `SETTINGS` |
| HTTP/2 concurrent streams | 256 | per connection | fixed, advertised in `SETTINGS` |
| Request queue between host and isolate | 1024 | per server | fixed |
| Response body chunks in flight | 8 | per response | fixed |

Timeouts are on by default and disabled per option with `null`. The connection
cap is off by default: the right number follows from a deployment's
file-descriptor budget and the memory a connection costs, neither of which the
runtime can read.

### Sizing a deployment

The read buffer is the number that multiplies. An HTTP/1.1 connection's buffer
can reach ~408KB, so ten thousand connections that are valid, idle, and doing
nothing is roughly 4GB the server will allocate on its way to the descriptor
limit. That is what `maxConnections` is for, and why it is worth setting on a
public port even though nothing forces you to.

The cap is enforced by **not accepting**. A permit is taken before `accept` and
released when the connection ends, so a connection over the limit waits in the
kernel's backlog and costs the server nothing at all — no descriptor, no task, no
buffer — until a slot frees, at which point it is *served*. It is a queue, not a
rejection. Once the backlog itself fills, the OS refuses further connections,
which is the only refusal in the design and it also costs the server nothing.

### HTTP/2 concurrency against the isolate

The 256-stream cap exists because JavaScript runs on one thread. An HTTP/2 peer
opens streams far faster than a single-threaded isolate answers them, and every
open stream holds a queued request plus its body channel. The cap bounds what
one connection can make the server hold, which leaves the 1024-slot request
queue for spreading across connections rather than being filled by one client.

## Protocol version

The version is the client's choice, decided per connection, and the handler
never sees which one carried a request.

Over TLS it is ALPN: `serve()` advertises `["h2", "http/1.1"]` — h2 first,
because ALPN order is the server's preference — and the client takes the first
it speaks. On a cleartext port it is the HTTP/2 connection preface: a connection
opening with those 24 bytes is read as **h2c by prior knowledge**, anything else
as HTTP/1.1. There is no `Upgrade:`-header dance; that mechanism is deprecated
and no client relies on it. This is what a reverse proxy, or a gRPC client with
TLS terminated in front of the runtime, speaks.

What changes on the wire, with the handler untouched:

| | |
| --- | --- |
| Multiplexing | many requests in flight on one connection, answered in any order |
| Handshakes | one TLS handshake per session, not per connection |
| Headers | HPACK-compressed instead of resent in full each request |
| `request.url` | rebuilt from `:authority`, which replaced the `Host` header |
| Framing | the version frames bodies; a handler's own `Content-Length` and `Transfer-Encoding` are dropped, and HTTP/2 forbids chunked encoding outright |

Multiplexing needed nothing new above the socket. Responses were already matched
to requests by id rather than by arrival order, so answering three streams out
of order is the same code path as answering three connections.

Whether HTTP/2 is *faster* depends entirely on how the client connects — on one
connection it is 3.65× here, across 50 connections it loses. That is measured
rather than assumed: see [Benchmarks](/docs/benchmarks#http2).

## Identity: who is calling

The handler's second argument carries `remoteAddr` — the other end of the
socket, and only ever that. Behind a reverse proxy it is the proxy.

`X-Forwarded-For` is never consulted. Resolving it requires knowing which hop to
trust, and a header anyone can send is not an identity until something says
whose to believe; a misconfigured trust list is a spoofable identity, which is
worse than no answer. The header is delivered untouched, so a deployment that
knows its own topology resolves it in one line:

```js
const client = request.headers.get("x-forwarded-for")?.split(",")[0].trim()
  ?? info.remoteAddr.hostname;
```

On HTTP/2 every request multiplexed onto one connection reports the same peer,
because they are one connection. There is no per-IP connection cap yet — one
peer can still take every slot of a `maxConnections` budget.

## How this compares

Measured on one machine on one day, by standing each server up and probing it —
not read from documentation. Reproduce with `bash bench/probe-runtimes.sh`.

<!-- BEGIN probe:table -->
| | esrun | Node.js | Bun | Deno |
| --- | --- | --- | --- | --- |
| Silent connection closed after | 10.0s | 88.1s | 13.0s | never (>150s) |
| Idle keep-alive closed after | 30.0s | 6.0s | 12.0s | never (>150s) |
| HTTP/2 concurrent streams | 256 | unlimited | unlimited | 200 |
| HTTP/2 header list | 16KB | unlimited | 64KB | 16KB |
| HTTP/2 initial window | 1MB | 64KB | 64KB | 1MB |

<sub>esrun 0.15.0 · Node 24.14.0 · Bun 1.4.0 · Deno 2.8.3 · Linux · 2026-08-03</sub>
<!-- END probe:table -->

A silent connection — one that completes the TCP handshake and then says nothing
— is the cheapest hold on a server there is: one syscall to the peer, no state to
keep. Node bounds it at 88s, which is a 60s `headersTimeout` polled on a 30s
interval. Deno does not bound it at all, despite being built on the same HTTP
implementation we are; nor does it bound an idle keep-alive connection.

The HTTP/2 rows are read off the wire from each server's `SETTINGS` frame, which
is what it tells every client its limits are. Node and Bun advertise unlimited
concurrent streams; the runtimes that cap are the two on hyper.

None of this makes one runtime better than another — Node's 5s keep-alive is
more aggressive than ours, and its 88s header bound is looser. It is here so
that a number in this documentation can be checked rather than believed.

## See also

- [`runtime:http` API reference](/api/http) — signatures, options, errors
- [HTTP server guide](/docs/http) — how to build one
- [Benchmarks](/docs/benchmarks) — throughput, including HTTP/1.1 vs HTTP/2
- [vs Node · Bun · Deno](/docs/comparison) — capability-level comparison
