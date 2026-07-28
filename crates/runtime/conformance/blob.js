// WinterTC §2.9 — Blob / File / FormData.
//
// Cases still written as `todo` are known deviations; see RESULTS.md.

test("Blob reports size and normalises type to lower case", () => {
  const b = new Blob(["hello"], { type: "TEXT/Plain" });
  assertEquals(b.size, 5);
  assertEquals(b.type, "text/plain");
});

test("Blob.slice honours negative and partial ranges", () => {
  const b = new Blob(["hello"]);
  assertEquals(b.slice(1, 3).size, 2);
  assertEquals(b.slice(-2).size, 2);
  assertEquals(b.slice().type, "");
});

test("Blob concatenates string, ArrayBuffer and view parts", () => {
  const b = new Blob(["ab", new Uint8Array([99]), new Uint8Array([100]).buffer]);
  assertEquals(b.size, 4);
});

test("File is a Blob and carries name and lastModified", () => {
  const f = new File(["x"], "n.txt", { lastModified: 42 });
  assert(f instanceof Blob);
  assertEquals(f.name, "n.txt");
  assertEquals(f.lastModified, 42);
});

test("File requires a name argument", () => {
  assertThrows(() => new File(["x"]), "TypeError");
});

test("Blob rejects a non-iterable blobParts argument", () => {
  assertThrows(() => new Blob(123), "TypeError");
  assertThrows(() => new Blob({}), "TypeError");
  assertThrows(() => new Blob("bare string"), "TypeError");
  // Any iterable of parts is accepted, and no argument means empty.
  assertEquals(new Blob().size, 0);
  assertEquals(new Blob(new Set(["ab", "c"])).size, 3);
});

test("Blob drops a type that is not a valid MIME type", () => {
  assertEquals(new Blob([], { type: "not a type" }).type, "");
  assertEquals(new Blob([], { type: "text" }).type, "");
  assertEquals(new Blob([], { type: "text/plain\u0001" }).type, "");
  assertEquals(new Blob([], { type: "t\u00ebxt/plain" }).type, "");
  // Valid types, including parameters, are kept and lower-cased.
  assertEquals(new Blob([], { type: "TEXT/Plain" }).type, "text/plain");
  assertEquals(
    new Blob([], { type: "text/plain;charset=UTF-8" }).type,
    "text/plain;charset=utf-8",
  );
  // slice() validates its contentType the same way.
  assertEquals(new Blob(["x"]).slice(0, 1, "not a type").type, "");
  assertEquals(new Blob(["x"]).slice(0, 1, "text/plain").type, "text/plain");
});

test("Blob honours endings: 'native'", () => {
  // "\r\n" normalises to the platform newline; on unix that is one byte.
  assertEquals(new Blob(["a\r\nb"], { endings: "native" }).size, 3);
  assertEquals(new Blob(["a\rb"], { endings: "native" }).size, 3);
  // The default, "transparent", leaves the bytes alone.
  assertEquals(new Blob(["a\r\nb"]).size, 4);
  // Only string parts are normalised.
  assertEquals(
    new Blob([new Uint8Array([0x61, 0x0d, 0x0a])], { endings: "native" }).size,
    3,
  );
});

test("File exposes webkitRelativePath", () => {
  assertEquals(new File(["x"], "n.txt").webkitRelativePath, "");
});

// ---- FormData -------------------------------------------------------------

test("FormData append keeps insertion order and getAll returns every value", () => {
  const fd = new FormData();
  fd.append("a", "1");
  fd.append("a", "2");
  fd.append("b", "3");
  assertEquals(fd.getAll("a").join(","), "1,2");
  assertEquals([...fd.keys()].join(","), "a,a,b");
});

test("FormData set replaces the first entry in place and drops the rest", () => {
  const fd = new FormData();
  fd.append("a", "1");
  fd.append("b", "2");
  fd.append("a", "3");
  fd.set("a", "9");
  assertEquals(fd.getAll("a").join(","), "9");
  assertEquals([...fd.keys()].join(","), "a,b");
});

test("FormData get returns null for a missing name and coerces names", () => {
  const fd = new FormData();
  fd.append(null, "v");
  assertEquals(fd.get("missing"), null);
  assertEquals(fd.get("null"), "v");
});

test("FormData wraps a Blob value in a File when a filename is given", () => {
  const fd = new FormData();
  fd.append("f", new Blob(["x"]), "given.txt");
  const v = fd.get("f");
  assert(v instanceof File);
  assertEquals(v.name, "given.txt");
});

test("FormData delete and has operate on every matching entry", () => {
  const fd = new FormData();
  fd.append("a", "1");
  fd.append("a", "2");
  assertEquals(fd.has("a"), true);
  fd.delete("a");
  assertEquals(fd.has("a"), false);
});
