// WebIDL interface shape — the rules every platform class shares, rather than
// any one API's behaviour: branding (`Symbol.toStringTag`), what is allowed to
// appear on a public prototype, constructor arity, and named iterator objects.
//
// Cases still written as `todo` are known deviations; see RESULTS.md.

const tagOf = (v) => Object.prototype.toString.call(v);

// ---- Symbol.toStringTag branding -----------------------------------------

test("DOMException is branded", () => {
  assertEquals(tagOf(new DOMException("m", "AbortError")), "[object DOMException]");
});

test("Blob and File are branded", () => {
  assertEquals(tagOf(new Blob(["x"])), "[object Blob]");
  assertEquals(tagOf(new File(["x"], "f.txt")), "[object File]");
});

test("FormData is branded", () => {
  assertEquals(tagOf(new FormData()), "[object FormData]");
});

test("URL and URLSearchParams are branded", () => {
  assertEquals(tagOf(new URL("https://a.example/")), "[object URL]");
  assertEquals(tagOf(new URLSearchParams("a=1")), "[object URLSearchParams]");
});

test("Event, CustomEvent and EventTarget are branded", () => {
  assertEquals(tagOf(new Event("x")), "[object Event]");
  assertEquals(tagOf(new CustomEvent("x")), "[object CustomEvent]");
  assertEquals(tagOf(new EventTarget()), "[object EventTarget]");
});

test("AbortController and AbortSignal are branded", () => {
  assertEquals(tagOf(new AbortController()), "[object AbortController]");
  assertEquals(tagOf(AbortSignal.abort()), "[object AbortSignal]");
});

test("TextEncoder and TextDecoder are branded", () => {
  assertEquals(tagOf(new TextEncoder()), "[object TextEncoder]");
  assertEquals(tagOf(new TextDecoder()), "[object TextDecoder]");
});

test("the stream classes are branded", () => {
  assertEquals(tagOf(new ReadableStream()), "[object ReadableStream]");
  assertEquals(tagOf(new WritableStream()), "[object WritableStream]");
  assertEquals(tagOf(new TransformStream()), "[object TransformStream]");
});

test("the fetch interfaces are branded", () => {
  assertEquals(tagOf(new Headers()), "[object Headers]");
  assertEquals(tagOf(new Request("https://a.example/")), "[object Request]");
  assertEquals(tagOf(new Response("x")), "[object Response]");
});

test("the transform-stream interfaces are branded", () => {
  assertEquals(tagOf(new TextEncoderStream()), "[object TextEncoderStream]");
  assertEquals(tagOf(new TextDecoderStream()), "[object TextDecoderStream]");
  assertEquals(tagOf(new CompressionStream("gzip")), "[object CompressionStream]");
  assertEquals(tagOf(new DecompressionStream("gzip")), "[object DecompressionStream]");
});

test("the queuing strategies and URLPattern are branded", () => {
  assertEquals(tagOf(new CountQueuingStrategy({ highWaterMark: 1 })), "[object CountQueuingStrategy]");
  assertEquals(
    tagOf(new ByteLengthQueuingStrategy({ highWaterMark: 1 })),
    "[object ByteLengthQueuingStrategy]",
  );
  assertEquals(tagOf(new URLPattern({ pathname: "/a" })), "[object URLPattern]");
});

test("branding is a non-enumerable, configurable prototype property", () => {
  const d = Object.getOwnPropertyDescriptor(Blob.prototype, Symbol.toStringTag);
  assertEquals(d.value, "Blob");
  assertEquals(d.enumerable, false);
  assertEquals(d.writable, false);
  assertEquals(d.configurable, true);
  // On the prototype, not the instance.
  assertEquals(Object.getOwnPropertySymbols(new Blob([])).length, 0);
});

test("crypto and performance are branded", () => {
  assertEquals(tagOf(crypto), "[object Crypto]");
  assertEquals(tagOf(crypto.subtle), "[object SubtleCrypto]");
  assertEquals(tagOf(performance), "[object Performance]");
});

test("crypto and performance members live on their prototypes", () => {
  for (const [obj, name] of [
    [crypto, "getRandomValues"],
    [crypto, "randomUUID"],
    [crypto.subtle, "digest"],
    [performance, "now"],
  ]) {
    assertEquals(Object.prototype.hasOwnProperty.call(obj, name), false);
    assertEquals(typeof Object.getPrototypeOf(obj)[name], "function");
  }
});

test("the Crypto and Performance constructors are exposed but not callable", () => {
  assertEquals(typeof Crypto, "function");
  assertEquals(typeof SubtleCrypto, "function");
  assertEquals(typeof Performance, "function");
  assert(crypto instanceof Crypto);
  assert(crypto.subtle instanceof SubtleCrypto);
  assert(performance instanceof Performance);
  assertThrows(() => new Crypto(), "TypeError");
  assertThrows(() => new SubtleCrypto(), "TypeError");
  assertThrows(() => new Performance(), "TypeError");
});

// ---- Internal plumbing must not sit on public prototypes ------------------

const noUnderscored = (name, proto) => {
  const leaked = Object.getOwnPropertyNames(proto).filter((k) => k.startsWith("_"));
  assertEquals(leaked.join(","), "", `${name} leaks internals`);
};

test("Blob.prototype exposes no internal members", () => {
  noUnderscored("Blob", Blob.prototype);
});

test("Event.prototype exposes no internal members", () => {
  noUnderscored("Event", Event.prototype);
});

test("URL and URLSearchParams expose no internal members", () => {
  noUnderscored("URL", URL.prototype);
  noUnderscored("URLSearchParams", URLSearchParams.prototype);
});

test("the fetch interfaces expose no internal members", () => {
  noUnderscored("Headers", Headers.prototype);
  noUnderscored("Request", Request.prototype);
  noUnderscored("Response", Response.prototype);
});

test("FormData exposes no internal members", () => {
  noUnderscored("FormData", FormData.prototype);
});

// ---- Constructor arity (optional arguments are not counted) ---------------

test("Event and File constructor lengths match WebIDL", () => {
  assertEquals(Event.length, 1);
  assertEquals(File.length, 2);
  assertEquals(Blob.length, 0);
});

test("URL and URLSearchParams constructor lengths match WebIDL", () => {
  assertEquals(URL.length, 1);
  assertEquals(URLSearchParams.length, 0);
});

// ---- Named iterator objects ----------------------------------------------

test("URLSearchParams iterators are named, not bare generators", () => {
  assertEquals(tagOf(new URLSearchParams("a=1").entries()), "[object URLSearchParams Iterator]");
});

test("FormData iterators are named, not bare generators", () => {
  assertEquals(tagOf(new FormData().entries()), "[object FormData Iterator]");
});

test("Headers iterators are named, not bare generators", () => {
  assertEquals(tagOf(new Headers().entries()), "[object Headers Iterator]");
  assertEquals(tagOf(new Headers().keys()), "[object Headers Iterator]");
  assertEquals(tagOf(new Headers().values()), "[object Headers Iterator]");
});

test("named iterators still iterate and inherit %IteratorPrototype%", () => {
  const sp = new URLSearchParams("a=1&b=2");
  assertEquals([...sp.entries()].map(([k, v]) => k + v).join(","), "a1,b2");
  assertEquals([...sp.keys()].join(","), "a,b");
  assertEquals([...sp].length, 2);
  const it = sp.entries();
  // %IteratorPrototype% supplies [Symbol.iterator], so an iterator is iterable.
  assertEquals(it[Symbol.iterator](), it);
  const h = new Headers({ "x-a": "1" });
  assertEquals([...h].map(([k, v]) => k + v).join(","), "x-a1");
  const fd = new FormData();
  fd.append("a", "1");
  assertEquals([...fd.values()].join(","), "1");
});

// ---- Illegal invocation ---------------------------------------------------

test("platform constructors reject being called without new", () => {
  for (const C of [Event, Blob, URL, EventTarget, TextEncoder]) {
    assertThrows(() => C("x"), "TypeError");
  }
});

test("the messaging interfaces are branded and exposed", () => {
  const ch = new MessageChannel();
  assertEquals(tagOf(ch), "[object MessageChannel]");
  assertEquals(tagOf(ch.port1), "[object MessagePort]");
  const bc = new BroadcastChannel("brand-check");
  assertEquals(tagOf(bc), "[object BroadcastChannel]");
  bc.close();
});

test("runtime internals are not enumerable on the global object", () => {
  // `Object.keys(globalThis)` and `for (const k in globalThis)` listed
  // `__wasm_pending`, `__structuredSerialize` and friends beside `fetch` and
  // `console` — non-standard surface for any code that walks the globals.
  const internals = [
    "__ops",
    "__internal",
    "__wasm_pending",
    "__wasm_module",
    "__structuredSerialize",
    "__structuredDeserialize",
    "__responseTrailers",
    "__serverRequest",
    "__make_import_meta_resolve",
    "__dispatch_error_event",
    "__dispatch_unhandled_rejection",
    "__dispatch_rejection_handled",
    "__structuredWriteHostObject",
    "__structuredReadHostObject",
  ];
  const keys = new Set(Object.keys(globalThis));
  const leaked = internals.filter((n) => keys.has(n));
  assertEquals(leaked.length, 0, `enumerable internals: ${leaked.join(" ")}`);
  // They are still present and still work — this is presentation, not removal.
  assertEquals(typeof globalThis.__structuredSerialize, "function");
  assertEquals(structuredClone({ a: 1 }).a, 1);
});

test("runtime:system exposes no implementation details on its prototypes", async () => {
  const { Command, ChildProcess } = await import("runtime:system");
  // `_collect`, `_readable`, `_streamDone` and friends were public members of
  // the documented API surface.
  const own = (o) => Object.getOwnPropertyNames(o).filter((k) => k !== "constructor");
  assertEquals(own(Command.prototype).sort().join(","), "output,spawn");
  assertEquals(own(ChildProcess.prototype).sort().join(","), "kill,status,stderr,stdin,stdout");
});
