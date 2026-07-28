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

todo("Blob and File are branded", () => {
  assertEquals(tagOf(new Blob(["x"])), "[object Blob]");
  assertEquals(tagOf(new File(["x"], "f.txt")), "[object File]");
});

todo("FormData is branded", () => {
  assertEquals(tagOf(new FormData()), "[object FormData]");
});

todo("URL and URLSearchParams are branded", () => {
  assertEquals(tagOf(new URL("https://a.example/")), "[object URL]");
  assertEquals(tagOf(new URLSearchParams("a=1")), "[object URLSearchParams]");
});

todo("Event, CustomEvent and EventTarget are branded", () => {
  assertEquals(tagOf(new Event("x")), "[object Event]");
  assertEquals(tagOf(new CustomEvent("x")), "[object CustomEvent]");
  assertEquals(tagOf(new EventTarget()), "[object EventTarget]");
});

todo("AbortController and AbortSignal are branded", () => {
  assertEquals(tagOf(new AbortController()), "[object AbortController]");
  assertEquals(tagOf(AbortSignal.abort()), "[object AbortSignal]");
});

todo("TextEncoder and TextDecoder are branded", () => {
  assertEquals(tagOf(new TextEncoder()), "[object TextEncoder]");
  assertEquals(tagOf(new TextDecoder()), "[object TextDecoder]");
});

todo("the stream classes are branded", () => {
  assertEquals(tagOf(new ReadableStream()), "[object ReadableStream]");
  assertEquals(tagOf(new WritableStream()), "[object WritableStream]");
  assertEquals(tagOf(new TransformStream()), "[object TransformStream]");
});

todo("crypto and performance are branded", () => {
  assertEquals(tagOf(crypto), "[object Crypto]");
  assertEquals(tagOf(crypto.subtle), "[object SubtleCrypto]");
  assertEquals(tagOf(performance), "[object Performance]");
});

// ---- Internal plumbing must not sit on public prototypes ------------------

const noUnderscored = (name, proto) => {
  const leaked = Object.getOwnPropertyNames(proto).filter((k) => k.startsWith("_"));
  assertEquals(leaked.join(","), "", `${name} leaks internals`);
};

todo("Blob.prototype exposes no internal members", () => {
  noUnderscored("Blob", Blob.prototype);
});

todo("Event.prototype exposes no internal members", () => {
  noUnderscored("Event", Event.prototype);
});

todo("URL and URLSearchParams expose no internal members", () => {
  noUnderscored("URL", URL.prototype);
  noUnderscored("URLSearchParams", URLSearchParams.prototype);
});

// ---- Constructor arity (optional arguments are not counted) ---------------

test("Event and File constructor lengths match WebIDL", () => {
  assertEquals(Event.length, 1);
  assertEquals(File.length, 2);
  assertEquals(Blob.length, 0);
});

todo("URL and URLSearchParams constructor lengths match WebIDL", () => {
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
