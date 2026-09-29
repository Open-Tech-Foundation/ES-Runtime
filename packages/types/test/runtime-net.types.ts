// A type test for `runtime:net`'s Unix domain sockets (DECISIONS D140).
//
// Same reasoning as `runtime-test.types.ts`: `@ts-expect-error` fails the build
// when the error it names stops happening, so an overload that quietly widened
// breaks this file rather than passing it.

import type { Listener, Socket, UnixListener } from "runtime:net";
import { connect, listen } from "runtime:net";

// --- a path replaces the host and port ------------------------------------------

const socket: Socket = connect({ path: "/var/run/docker.sock" });
const halfOpen: Socket = connect({ path: "/run/app.sock" }, { allowHalfOpen: true });

const server: UnixListener = listen({ path: "/run/app.sock" });
const where: Promise<{ path: string }> = server.addr;

// A TCP listener still reports a host and a port.
const tcp: Listener = listen({ hostname: "127.0.0.1", port: 0 });
const tcpAddr: Promise<{ hostname: string; port: number }> = tcp.addr;

// Accepted sockets are ordinary sockets.
for await (const accepted of server) {
  const s: Socket = accepted;
  void s;
}

// --- refused ---------------------------------------------------------------------

// @ts-expect-error — a Unix socket is plaintext.
connect({ path: "/run/app.sock" }, { secureTransport: "on" });

// @ts-expect-error — a Unix listener has no path-less address.
const wrong: Promise<{ hostname: string; port: number }> = server.addr;

void [socket, halfOpen, where, tcpAddr, wrong];
