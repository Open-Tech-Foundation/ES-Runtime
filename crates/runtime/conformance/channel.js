// HTML messaging — MessageChannel / MessagePort / BroadcastChannel.
//
// Delivery is asynchronous (a queued task), so the ordering and buffering
// behaviour is gated by Rust tests that drive the event loop. What is asserted
// here is everything observable without running the loop.

test("MessageChannel exposes two entangled ports", () => {
  const ch = new MessageChannel();
  assert(ch.port1 instanceof MessagePort);
  assert(ch.port2 instanceof MessagePort);
  assert(ch.port1 !== ch.port2);
  // The accessors are stable, not fresh objects each read.
  assertEquals(ch.port1, ch.port1);
  assert(ch.port1 instanceof EventTarget);
});

test("MessagePort is not constructible from script", () => {
  assertThrows(() => new MessagePort(), "TypeError");
});

test("postMessage clones synchronously, so a later mutation is not sent", () => {
  const ch = new MessageChannel();
  const payload = { n: 1 };
  ch.port1.postMessage(payload);
  payload.n = 2;
  // The clone happened at the call; nothing observable here beyond it not
  // throwing, but a non-cloneable value must throw at the call site.
  assertThrows(() => ch.port1.postMessage(() => {}), "DataCloneError");
});

test("postMessage on a closed or unpaired port is a no-op", () => {
  const ch = new MessageChannel();
  ch.port1.close();
  ch.port1.postMessage("x");
  // Closing one end disentangles the pair, so the peer's send drops too.
  ch.port2.postMessage("y");
});

test("a MessagePort transfer list is rejected", () => {
  const ch = new MessageChannel();
  const other = new MessageChannel();
  assertThrows(
    () => ch.port1.postMessage("x", [other.port1]),
    "DataCloneError",
  );
});

test("BroadcastChannel requires a name and reports it", () => {
  assertThrows(() => new BroadcastChannel(), "TypeError");
  const bc = new BroadcastChannel("room");
  assertEquals(bc.name, "room");
  assert(bc instanceof EventTarget);
  bc.close();
});

test("BroadcastChannel.postMessage after close is an InvalidStateError", () => {
  const bc = new BroadcastChannel("closed-room");
  bc.close();
  assertThrows(() => bc.postMessage("x"), "InvalidStateError");
  // close() is idempotent.
  bc.close();
});

test("BroadcastChannel rejects a non-cloneable message", () => {
  const bc = new BroadcastChannel("clone-room");
  assertThrows(() => bc.postMessage(Symbol("nope")), "DataCloneError");
  bc.close();
});
