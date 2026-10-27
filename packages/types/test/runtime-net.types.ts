// A type test for `runtime:net`'s Unix domain sockets (DECISIONS D140) and
// `resolveDns` (D188).
//
// Same reasoning as `runtime-test.types.ts`: `@ts-expect-error` fails the build
// when the error it names stops happening, so an overload that quietly widened
// breaks this file rather than passing it.

import type { Listener, MxRecord, Socket, SrvRecord, UnixListener } from "runtime:net";
import { connect, listen, resolveDns } from "runtime:net";

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

// --- resolveDns: the record shape follows the type --------------------------------

const addresses: Promise<string[]> = resolveDns("example.com", "A");
const mx: Promise<MxRecord[]> = resolveDns("example.com", "MX");
const txt: Promise<string[][]> = resolveDns("example.com", "TXT");
const srv: Promise<SrvRecord[]> = resolveDns("_mongodb._tcp.example.com", "SRV", {
  nameServer: { ipAddr: "10.0.0.2" },
  signal: AbortSignal.timeout(1000),
});

// @ts-expect-error — TXT records are arrays of strings, not strings.
const flatTxt: Promise<string[]> = resolveDns("example.com", "TXT");

// @ts-expect-error — ANY is not a type resolveDns asks for.
resolveDns("example.com", "ANY");

// @ts-expect-error — a name server is an address object, not a string.
resolveDns("example.com", "A", { nameServer: "10.0.0.2" });

void [socket, halfOpen, where, tcpAddr, wrong, addresses, mx, txt, srv, flatTxt];
