import { cacheControl, contentType, isAssetName } from "./assets.ts";

test("a content type is chosen by extension, case-insensitively", () => {
  assertEquals(contentType("entry.client-a1b2c3d4.js"), "text/javascript; charset=utf-8");
  assertEquals(contentType("app-9f8e7d6c.css"), "text/css; charset=utf-8");
  assertEquals(contentType("logo.SVG"), "image/svg+xml");
  assertEquals(contentType("inter.woff2"), "font/woff2");
});

test("a text type carries a charset", () => {
  // Without it the browser falls back to its locale's encoding, and the first
  // non-ASCII character in the file is where you find out.
  for (const name of ["a.js", "a.css", "a.json", "a.html"]) {
    assert(contentType(name).includes("charset=utf-8"), `${name}: ${contentType(name)}`);
  }
});

test("an unknown extension is not guessed at", () => {
  // `application/octet-stream` makes a browser download the file. Guessing
  // `text/html` for something a user put in the directory is how a static file
  // becomes a stored cross-site scripting bug.
  assertEquals(contentType("archive.tar.zst"), "application/octet-stream");
  assertEquals(contentType("noextension"), "application/octet-stream");
  assertEquals(contentType(".gitignore"), "application/octet-stream");
});

test("an asset name is one path segment and nothing else", () => {
  assert(isAssetName("entry.client-a1b2c3d4.js"));
  assert(isAssetName("logo.svg"));

  // The traversals. `URL` has already percent-decoded the path by the time a
  // name reaches here, so `%2e%2e%2f` arrives as `../` and is caught by the
  // same rule that catches `../` written plainly.
  assert(!isAssetName("../server.js"), "climbed out with ..");
  assert(!isAssetName("..\\server.js"), "climbed out with a backslash");
  assert(!isAssetName("nested/thing.js"), "named a subdirectory");
  assert(!isAssetName(".."), "named the parent directly");
  assert(!isAssetName("evil\0.js"), "smuggled a NUL");
  assert(!isAssetName(""), "named nothing");
});

test("an implausibly long name is refused before it reaches the filesystem", () => {
  assert(!isAssetName("a".repeat(300)));
});

test("only a hashed build may be cached for ever", () => {
  // Getting this wrong in the cacheable direction cannot be taken back: the
  // browsers that accepted the answer are not reachable to correct it.
  assert(cacheControl(false).includes("immutable"), cacheControl(false));
  assert(!cacheControl(true).includes("immutable"), cacheControl(true));
});
