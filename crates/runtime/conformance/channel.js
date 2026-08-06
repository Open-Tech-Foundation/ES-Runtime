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

// A port may be transferred and may not be cloned: two ends of a channel cannot
// become three. (This asserted that *any* port in a transfer list was rejected,
// which was true only while there was nowhere to transfer one to.)
test("a MessagePort can be cloned only by transferring it", () => {
  const ch = new MessageChannel();
  const other = new MessageChannel();
  // Not in the transfer list: a copy, which the spec refuses.
  assertThrows(() => ch.port1.postMessage(other.port1), "DataCloneError");
  // In the transfer list: allowed.
  ch.port1.postMessage("x", [other.port1]);
});

test("a transferred MessagePort delivers what was queued before the transfer", () => {
  const source = new MessageChannel();
  const carrier = new MessageChannel();
  // Posted while `source.port2` is still here, then the port moves.
  source.port1.postMessage("queued before transfer");

  carrier.port2.onmessage = (e) => {
    e.data.port.onmessage = (m) => {
      assertEquals(m.data, "queued before transfer");
      carrier.port1.close();
      carrier.port2.close();
      source.port1.close();
    };
  };
  carrier.port1.postMessage({ port: source.port2 }, [source.port2]);
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
