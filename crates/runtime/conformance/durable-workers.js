// runtime:workers — the rules a durable worker enforces before it touches a
// disk (DECISIONS D80).
//
// Everything here is observable without opening a database, which is the same
// split `workers.js` makes for the HTML `Worker`: materializing one, running a
// call and surviving a restart all need a real filesystem and a real loop, and
// are covered end-to-end by `crates/runtime-cli/tests/durable_workers.rs`
// against the real CLI.
//
// What that leaves is the part a program can get wrong on the first line: an
// argument that is not an id, a class with no usable storage name, a reference
// mistaken for a promise. Each of those has to fail at the call that made it
// rather than three awaits later, and each is a rule somebody could otherwise
// trip silently.
//
// The module is imported inside each case rather than at the top: these files
// are evaluated as scripts, where there is no top-level await.

test("a durable worker is addressed, not constructed", async () => {
  const { DurableWorker } = await import("runtime:workers");
  class Sample extends DurableWorker {}
  assertThrows(() => new Sample(), "TypeError");
  assertThrows(() => new DurableWorker(), "TypeError");
});

test("the base class is a class, and instances say what they are", async () => {
  const { DurableWorker } = await import("runtime:workers");
  class Sample extends DurableWorker {}
  assert(typeof DurableWorker === "function");
  assertEquals(DurableWorker.prototype[Symbol.toStringTag], "DurableWorker");
  assert(Sample.prototype instanceof DurableWorker);
});

// `get` is the address book, and addressing costs nothing: no file is opened
// until a method is called, so an id that cannot name a worker must be refused
// here rather than at the first call.
test("an id must be a non-empty string", async () => {
  const { DurableWorker } = await import("runtime:workers");
  class Ids extends DurableWorker {}
  assertThrows(() => Ids.get(), "TypeError");
  assertThrows(() => Ids.get(7), "TypeError");
  assertThrows(() => Ids.get(""), "TypeError");
  assertThrows(() => Ids.get("x".repeat(513)), "TypeError");
});

test("a reference carries its id and forwards methods", async () => {
  const { DurableWorker } = await import("runtime:workers");
  class Refs extends DurableWorker {
    async ping() {
      return "pong";
    }
  }
  const ref = Refs.get("one");
  assertEquals(ref.id, "one");
  assertEquals(typeof ref.ping, "function");
  assertEquals(typeof ref.anythingElse, "function"); // resolved at the call
});

// A reference that answered to `then` would be awaited *as a promise* — and
// `await ref` would call a method named `then` on the worker, which is a hang
// wearing an await's clothes.
test("a reference is not a thenable, and is read-only", async () => {
  const { DurableWorker } = await import("runtime:workers");
  class Thenable extends DurableWorker {}
  const ref = Thenable.get("one");
  assertEquals(ref.then, undefined);
  assertEquals(ref.catch, undefined);
  assertEquals(ref.finally, undefined);
  assertThrows(() => {
    ref.ping = 1;
  }, "TypeError");
});

// The storage name is a directory name and the key a state is filed under, so
// it is checked at the first use rather than discovered when a file cannot be
// written.
test("a storage name must be a plain identifier", async () => {
  const { DurableWorker } = await import("runtime:workers");
  class Bad extends DurableWorker {
    static durableName = "not a name";
  }
  assertThrows(() => Bad.get("x"), "TypeError");
});

test("two classes cannot store under one name", async () => {
  const { DurableWorker } = await import("runtime:workers");
  class First extends DurableWorker {
    static durableName = "shared_name";
  }
  class Second extends DurableWorker {
    static durableName = "shared_name";
  }
  First.get("x");
  assertThrows(() => Second.get("x"), "TypeError");
});

// An unknown option is a typo, and a typo that is skipped is a setting somebody
// believes they made. The same rule `new Worker`'s permissions follow.
test("configure refuses what it does not know", async () => {
  const { configure } = await import("runtime:workers");
  assertThrows(() => configure({ evictafter: 10 }), "TypeError");
  assertThrows(() => configure({ evictAfter: -1 }), "TypeError");
  assertThrows(() => configure({ mailbox: "many" }), "TypeError");
  assertThrows(() => configure({ valueLimit: 4096, stateLimit: 1024 }), "TypeError");
});

// The scheduler will not guess which classes this process is responsible for,
// and each entry has to be one — both checked at the call, since a scheduler
// that started with a bad list would simply never fire anything.
test("startAlarms must be told which classes it runs", async () => {
  const { startAlarms } = await import("runtime:workers");
  assertThrows(() => startAlarms(), "TypeError");
  assertThrows(() => startAlarms({}), "TypeError");
  assertThrows(() => startAlarms({ classes: [] }), "TypeError");
  assertThrows(() => startAlarms({ classes: [class NotOne {}] }), "TypeError");
});

test("startAlarms rejects an onError that is not a function", async () => {
  const { DurableWorker, startAlarms } = await import("runtime:workers");
  class Alarming extends DurableWorker {
    async alarm() {}
  }
  assertThrows(() => startAlarms({ classes: [Alarming], onError: "log" }), "TypeError");
});

// A schema is a literal in the source. Parsing it at `get()` means a mistake is
// reported by the line that addressed the worker, before any file is opened.
test("a schema is checked where it is written", async () => {
  const { DurableWorker } = await import("runtime:workers");
  class NotAnObject extends DurableWorker {
    static schema = "messages";
  }
  class WrongKey extends DurableWorker {
    static schema = { collection: {} };
  }
  class BadName extends DurableWorker {
    static schema = { collections: { "not a name": {} } };
  }
  class BadField extends DurableWorker {
    static schema = { collections: { ok: { index: ["a field"] } } };
  }
  class TakenColumn extends DurableWorker {
    static schema = { collections: { ok: { index: ["doc"] } } };
  }
  for (const cls of [NotAnObject, WrongKey, BadName, BadField, TakenColumn]) {
    assertThrows(() => cls.get("x"), "TypeError");
  }
});

test("a declared schema is accepted", async () => {
  const { DurableWorker } = await import("runtime:workers");
  class Fine extends DurableWorker {
    static schema = { collections: { messages: { index: ["ts"], unique: ["clientId"] } } };
  }
  assertEquals(Fine.get("room").id, "room");
});
