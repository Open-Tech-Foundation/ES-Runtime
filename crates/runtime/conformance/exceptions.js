// Engine exception reconciliation tests.

test("engine-thrown DOMException is a true DOMException instance (NotSupportedError)", async () => {
  let caught = null;
  try {
    await crypto.subtle.digest("UNKNOWN", new Uint8Array());
  } catch (e) {
    caught = e;
  }
  
  assertEquals(caught !== null, true);
  assertEquals(caught instanceof DOMException, true);
  assertEquals(caught.name, "NotSupportedError");
  assertEquals(caught.message.includes("unsupported digest"), true);
});

test("engine-thrown DOMException with a different name (DataError)", async () => {
  let caught = null;
  try {
    // Importing invalid key material throws a DataError from the native op
    await crypto.subtle.importKey("spki", new Uint8Array([1, 2, 3]), { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, true, ["verify"]);
  } catch (e) {
    caught = e;
  }
  
  assertEquals(caught !== null, true);
  assertEquals(caught instanceof DOMException, true);
  assertEquals(caught.name, "DataError");
});

test("engine-thrown native TypeError is a true TypeError instance", async () => {
  let caught = null;
  try {
    // Passing a number where a string is expected
    await crypto.subtle.digest(123, new Uint8Array());
  } catch (e) {
    caught = e;
  }
  
  assertEquals(caught !== null, true);
  assertEquals(caught instanceof TypeError, true);
  assertEquals(caught.name, "TypeError");
});

test("engine-thrown DOMException with OperationError name", async () => {
  let caught = null;
  try {
    // AES-GCM decrypt with corrupted ciphertext throws OperationError
    const key = await crypto.subtle.generateKey({ name: "AES-GCM", length: 128 }, false, ["encrypt", "decrypt"]);
    const iv = new Uint8Array(12);
    const ct = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, new Uint8Array([1, 2, 3]));
    // Corrupt the ciphertext tag
    const corrupted = new Uint8Array(ct);
    corrupted[corrupted.length - 1] ^= 1;
    await crypto.subtle.decrypt({ name: "AES-GCM", iv }, key, corrupted);
  } catch (e) {
    caught = e;
  }
  
  assertEquals(caught !== null, true);
  assertEquals(caught instanceof DOMException, true);
  assertEquals(caught.name, "OperationError");
});
