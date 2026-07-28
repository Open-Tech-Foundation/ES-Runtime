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

todo("URLSearchParams iterators are named, not bare generators", () => {
  assertEquals(tagOf(new URLSearchParams("a=1").entries()), "[object URLSearchParams Iterator]");
});

todo("FormData iterators are named, not bare generators", () => {
  assertEquals(tagOf(new FormData().entries()), "[object FormData Iterator]");
});

// ---- Illegal invocation ---------------------------------------------------

test("platform constructors reject being called without new", () => {
  for (const C of [Event, Blob, URL, EventTarget, TextEncoder]) {
    assertThrows(() => C("x"), "TypeError");
  }
});
